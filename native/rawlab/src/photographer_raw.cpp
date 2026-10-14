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
#include <cmath>
#include <cstdio>
#include <memory>
#include <stdexcept>
#include <type_traits>
using namespace sony2fuji;
static_assert(std::is_standard_layout<RGB>::value && sizeof(RGB) == 3 * sizeof(float), "RGB layout must match the float ABI");
static_assert(offsetof(RGB, r) == 0 && offsetof(RGB, g) == sizeof(float) && offsetof(RGB, b) == 2 * sizeof(float), "RGB channel offsets changed");
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
bool gpu_render(const ImageData &source, const std::shared_ptr<LUT3D> &lut, float strength, ImageData &output) {
    sony2fuji_request request{};
    request.version = SONY2FUJI_REQUEST_VERSION;
    request.struct_size = sizeof(request);
    request.input_type = SONY2FUJI_INPUT_BUFFER;
    request.lut_strength = strength;
    request.brightness = request.contrast = request.saturation = 1;
#ifdef PC_D3D11
    D3D11PhotoRenderer renderer; // M0 one-shot; persistent host sessions follow
    return renderer.render(source, ColorSpace::sRGB, request, lut, RGB(1, 1, 1), source.width, source.height, 0, output);
#elif defined(PC_METAL)
    return renderPhotoMetal(source, ColorSpace::sRGB, request, lut, RGB(1, 1, 1), source.width, source.height, output);
#elif defined(PC_GLES)
    GlesPhotoRenderer renderer;
    return renderer.render(source, ColorSpace::sRGB, request, lut, RGB(1, 1, 1), source.width, source.height, 0, output);
#else
    return false;
#endif
}
}
extern "C" uint32_t pc_raw_abi_version(void) { return 1; }
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
extern "C" int pc_raw_render(const float *rgb, uint32_t width, uint32_t height, const pc_film_options *options, pc_raw_result *result) {
    if (!result) return -1;
    *result = {};
    try {
        if (!rgb || !width || !height || uint64_t(width) * height > 128000000 || !options || options->version != 1 || !std::isfinite(options->strength) || options->strength < 0 || options->strength > 1 || options->gpu_mode > 2)
            throw std::runtime_error("Invalid RawLab film request");
        cancel(options->cancelled, options->userdata);
        auto image = std::make_unique<ImageData>(static_cast<int>(width), static_cast<int>(height));
        for (size_t i = 0; i < image->pixels.size(); ++i) {
            const float *p = rgb + i * 3;
            if (!std::isfinite(p[0]) || !std::isfinite(p[1]) || !std::isfinite(p[2])) throw std::runtime_error("Invalid linear pixel");
            image->pixels[i] = RGB(p[0], p[1], p[2]);
        }
        std::shared_ptr<LUT3D> lut;
        if (options->lut_path && *options->lut_path && options->strength > 0) {
            lut = LUTParser::loadLUTCached(options->lut_path);
            if (!lut || !lut->isPhotoLUT()) throw std::runtime_error("Unknown or unsupported film LUT color contract");
        }
        if (options->gpu_mode != 0) {
            auto output = std::make_unique<ImageData>();
            if (gpu_render(*image, lut, options->strength, *output)) {
                cancel(options->cancelled, options->userdata);
                result->source_width = width; result->source_height = height;
                result->gpu_used = 1;
                publish(std::move(output), 2, result);
                return 0;
            }
            if (options->gpu_mode == 2) throw std::runtime_error("RawLab platform GPU unavailable or failed");
        }
        ImageData neutral = *image;
        for (auto &p : neutral.pixels) p = RGB(neutralDisplay(p.r), neutralDisplay(p.g), neutralDisplay(p.b));
        if (lut) {
            if (lut->inputTransfer() == LUTTransfer::SRGB) *image = neutral;
            else encodePhotoLUTInput(*image, lut->inputTransfer());
            GpuConfig gpu;
            gpu.mode = GpuMode::Off; // whole-pipeline Auto already tried above
            check(applyLUTWithConfig(lut, *image, gpu));
            renderPhotoLUTOutput(*image, lut->outputTransfer());
            for (size_t i = 0; i < image->pixels.size(); ++i) {
                const auto &n = neutral.pixels[i]; auto &p = image->pixels[i];
                p = RGB(n.r + (p.r - n.r) * options->strength, n.g + (p.g - n.g) * options->strength, n.b + (p.b - n.b) * options->strength);
            }
        } else {
            *image = std::move(neutral);
        }
        cancel(options->cancelled, options->userdata);
        result->source_width = width; result->source_height = height;
        publish(std::move(image), 2, result);
        return 0;
    } catch (const std::exception &error) { return failure(result, error.what()); }
      catch (...) { return failure(result, "Unexpected RawLab film failure"); }
}
