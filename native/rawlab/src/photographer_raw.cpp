// SPDX-License-Identifier: GPL-3.0-only
// Float/lifetime bridge only. All RAW and film algorithms remain in RawLab.
#include "photographer_raw.h"
#include "sony2fuji/raw_processor.h"
#include "sony2fuji/lut_applicator.h"
#include "core/photo_lut.h"
#include "sony2fuji/gpu/lut_gpu.h"
#include "gpu/photo_gpu.h"
#ifdef PC_D3D11
#include "gpu/d3d11_photo.h"
#endif
#ifdef PC_GLES
#include "gpu/gles_photo.h"
#endif
#include <algorithm>
#include <atomic>
#include <filesystem>
#include <cmath>
#include <cstdio>
#include <memory>
#include <stdexcept>
#include <type_traits>
using namespace sony2fuji;
static_assert(std::is_standard_layout<RGB>::value && sizeof(RGB) == 3 * sizeof(float), "RGB layout must match the float ABI");
static_assert(offsetof(RGB, r) == 0 && offsetof(RGB, g) == sizeof(float) && offsetof(RGB, b) == 2 * sizeof(float), "RGB channel offsets changed");
struct pc_color_session {
    ImageData source;
    uint64_t revision;
#ifdef PC_D3D11
    D3D11PhotoRenderer renderer;
#elif defined(PC_GLES)
    GlesPhotoRenderer renderer;
#endif
};
namespace {
void check(ErrorCode status) {
    if (status != ErrorCode::Success) throw std::runtime_error("RawLab core failed (code " + std::to_string(static_cast<int>(status)) + ")");
}
void cancel(pc_raw_cancel callback, void *userdata) {
    if (callback && callback(userdata)) throw std::runtime_error("RawLab request cancelled");
}
void publish(std::unique_ptr<ImageData> image, uint32_t contract, pc_raw_result *result) {
    result->width = image->width; result->height = image->height;
    result->color_contract = contract;
    result->samples = image->pixels.size() * 3;
    result->rgb = reinterpret_cast<const float *>(image->pixels.data());
    result->owner = image.release();
}
int failure(pc_raw_result *result, const char *message) {
    *result = {};
    std::snprintf(result->error, sizeof(result->error), "%s", message);
    return -1;
}
std::shared_ptr<LUT3D> film_lut(const char *path) {
    if (!path || !*path || std::filesystem::file_size(std::filesystem::u8path(path)) > 16 * 1024 * 1024)
        throw std::runtime_error("Invalid film resource size");
    auto lut = LUTParser::loadLUTCached(path);
    if (!lut || !lut->isPhotoLUT() || lut->getSize() < 2 || lut->getSize() > 65)
        throw std::runtime_error("Film resource has no supported color contract or exceeds 65 grid");
    return lut;
}
std::unique_ptr<ImageData> cpu_render(const ImageData &source, const std::shared_ptr<LUT3D> &lut, const pc_film_options &options) {
    auto image = std::make_unique<ImageData>(source);
    const float gain = std::exp2(options.exposure_ev);
    for (auto &p : image->pixels) {
        p.r *= gain * options.relative_white_balance[0];
        p.g *= gain * options.relative_white_balance[1];
        p.b *= gain * options.relative_white_balance[2];
    }
    if (!lut) {
        for (auto &p : image->pixels) p = RGB(neutralDisplay(p.r), neutralDisplay(p.g), neutralDisplay(p.b));
        return image;
    }
    ImageData neutral = *image;
    for (auto &p : neutral.pixels) p = RGB(neutralDisplay(p.r), neutralDisplay(p.g), neutralDisplay(p.b));
    if (lut->inputTransfer() == LUTTransfer::SRGB) *image = neutral;
    else encodePhotoLUTInput(*image, lut->inputTransfer());
    GpuConfig gpu;
    gpu.mode = GpuMode::Off; // whole-pipeline Auto already tried above
    check(applyLUTWithConfig(lut, *image, gpu));
    renderPhotoLUTOutput(*image, lut->outputTransfer());
    for (size_t i = 0; i < image->pixels.size(); ++i) {
        const auto &n = neutral.pixels[i]; auto &p = image->pixels[i];
        p = RGB(n.r + (p.r - n.r) * options.strength, n.g + (p.g - n.g) * options.strength, n.b + (p.b - n.b) * options.strength);
    }
    return image;
}
bool gpu_render(pc_color_session &session, const std::shared_ptr<LUT3D> &lut, const pc_film_options &options, ImageData &output) {
    const auto &source = session.source;
    sony2fuji_request request{};
    request.version = SONY2FUJI_REQUEST_VERSION;
    request.struct_size = sizeof(request);
    request.input_type = SONY2FUJI_INPUT_RAW; // predecoded immutable source, cache revision below
    request.lut_strength = options.strength;
    request.exposure_ev = options.exposure_ev;
    const RGB wb(options.relative_white_balance[0], options.relative_white_balance[1], options.relative_white_balance[2]);
    request.brightness = request.contrast = request.saturation = 1;
#ifdef PC_D3D11
    return session.renderer.render(source, ColorSpace::sRGB, request, lut, wb, source.width, source.height, session.revision, output);
#elif defined(PC_METAL)
    return renderPhotoMetal(source, ColorSpace::sRGB, request, lut, wb, source.width, source.height, output);
#elif defined(PC_GLES)
    return session.renderer.render(source, ColorSpace::sRGB, request, lut, wb, source.width, source.height, session.revision, output);
#else
    return false;
#endif
}
}
extern "C" uint32_t pc_raw_abi_version(void) { return 2; }
extern "C" void pc_raw_free(void *owner) { delete static_cast<ImageData *>(owner); }
extern "C" int pc_raw_decode(const char *path, const pc_raw_options *options, pc_raw_result *result) {
    if (!result) return -1;
    *result = {};
    try {
        if (!path || !*path || !options || options->version != 1 || !options->max_pixels || options->half_size > 1)
            throw std::runtime_error("Invalid RawLab decode request");
        cancel(options->cancelled, options->userdata);
        RAWProcessor raw;
        check(raw.loadFile(path)); // core owns decode, calibration and file handles
        // Upstream loadFile unpacks before exposing geometry; this is an output
        // bound, not a pre-unpack memory guarantee. Host budget is still required.
        if (raw.getWidth() <= 0 || raw.getHeight() <= 0 || uint64_t(raw.getWidth()) * raw.getHeight() > options->max_pixels)
            throw std::runtime_error("RAW exceeds active pixel limit");
        cancel(options->cancelled, options->userdata);
        RAWProcessOptions request;
        request.halfSize = options->half_size != 0;
        request.applyBaselineExposure = false; // no implicit workflow +0.7 EV
        request.matchEmbeddedPreviewExposure = false;
        request.exposure = raw.getMetadataExposureEV(); // valid DNG baseline once
        const bool camera = std::all_of(options->white_balance, options->white_balance + 4, [](float v) { return v == 0; });
        if (!camera && !std::all_of(options->white_balance, options->white_balance + 4, [](float v) { return std::isfinite(v) && v > 0; }))
            throw std::runtime_error("Invalid sensor white balance");
        request.useCameraWhiteBalance = camera;
        request.useCustomWhiteBalance = !camera;
        if (!camera) std::copy_n(options->white_balance, 4, request.customWhiteBalance);
        auto image = std::make_unique<ImageData>();
        check(raw.process(request, *image));
        cancel(options->cancelled, options->userdata);
        if (image->width <= 0 || image->height <= 0 || uint64_t(image->width) * image->height > options->max_pixels)
            throw std::runtime_error("Invalid RawLab output dimensions");
        result->source_width = options->half_size ? 0 : image->width;
        result->source_height = options->half_size ? 0 : image->height;
        result->baseline_ev = request.exposure;
        publish(std::move(image), 1, result);
        return 0;
    } catch (const std::exception &error) { return failure(result, error.what()); }
      catch (...) { return failure(result, "Unexpected RawLab decode failure"); }
}
extern "C" int pc_color_session_create(const float *rgb, uint32_t width, uint32_t height, pc_color_session **out, pc_raw_result *diagnostics) {
    if (!out || !diagnostics) return -1;
    *out = nullptr; *diagnostics = {};
    try {
        if (!rgb || !width || !height || uint64_t(width) * height > 128000000) throw std::runtime_error("Invalid linear source dimensions");
        auto session = std::make_unique<pc_color_session>();
        session->source = ImageData(width, height);
        for (size_t i = 0; i < session->source.pixels.size(); ++i) {
            const float *p = rgb + i * 3;
            if (!std::isfinite(p[0]) || !std::isfinite(p[1]) || !std::isfinite(p[2])) throw std::runtime_error("Invalid linear pixel");
            session->source.pixels[i] = RGB(p[0], p[1], p[2]);
        }
        static std::atomic<uint64_t> revision{1};
        session->revision = revision.fetch_add(1);
        *out = session.release();
        return 0;
    } catch (const std::exception &error) { return failure(diagnostics, error.what()); }
      catch (...) { return failure(diagnostics, "Cannot create RawLab color session"); }
}
extern "C" void pc_color_session_destroy(pc_color_session *session) { delete session; }
extern "C" int pc_color_session_render(pc_color_session *session, const pc_film_options *options, pc_raw_result *result) {
    if (!result) return -1;
    *result = {};
    try {
        if (!session || !options || options->version != 2 || !std::isfinite(options->strength) || options->strength < 0 || options->strength > 1 || options->gpu_mode > 2 || !std::isfinite(options->exposure_ev) || std::abs(options->exposure_ev) > 20)
            throw std::runtime_error("Invalid RawLab color request");
        for (float gain : options->relative_white_balance) if (!std::isfinite(gain) || gain <= 0) throw std::runtime_error("Invalid relative white balance");
        cancel(options->cancelled, options->userdata);
        const uint32_t width = session->source.width, height = session->source.height;
        std::shared_ptr<LUT3D> lut;
        if (options->lut_path && *options->lut_path && options->strength > 0) {
            lut = film_lut(options->lut_path);
        }
        if (options->gpu_mode != 0) {
            auto output = std::make_unique<ImageData>();
            bool rendered = false;
            try { rendered = gpu_render(*session, lut, *options, *output); }
            catch (...) {
                // Auto must recover from device allocation/driver exceptions
                // too. CPU starts from the session's immutable linear source.
                if (options->gpu_mode == 2) throw;
            }
            if (rendered) {
                cancel(options->cancelled, options->userdata);
                result->source_width = width; result->source_height = height;
                result->gpu_used = 1;
                publish(std::move(output), 2, result);
                return 0;
            }
            if (options->gpu_mode == 2) throw std::runtime_error("RawLab platform GPU unavailable or failed");
        }
        auto image = cpu_render(session->source, lut, *options);
        cancel(options->cancelled, options->userdata);
        result->source_width = width; result->source_height = height;
        publish(std::move(image), 2, result);
        return 0;
    } catch (const std::exception &error) { return failure(result, error.what()); }
      catch (...) { return failure(result, "Unexpected RawLab film failure"); }
}

extern "C" int pc_raw_render(const float *rgb, uint32_t width, uint32_t height, const pc_film_options *options, pc_raw_result *result) {
    pc_color_session *source = nullptr;
    if (pc_color_session_create(rgb, width, height, &source, result) != 0) return -1;
    std::unique_ptr<pc_color_session> session(source);
    return pc_color_session_render(session.get(), options, result);
}
extern "C" int pc_raw_lut_info(const char *path, pc_lut_info *info, pc_raw_result *diagnostics) {
    if (!info || !diagnostics) return -1;
    *info = {}; *diagnostics = {};
    try {
        auto lut = film_lut(path);
        info->size = lut->getSize(); info->input_transfer = static_cast<uint32_t>(lut->inputTransfer()); info->output_transfer = static_cast<uint32_t>(lut->outputTransfer());
        return 0;
    } catch (const std::exception &error) { return failure(diagnostics, error.what()); }
      catch (...) { return failure(diagnostics, "Cannot inspect film resource"); }
}
