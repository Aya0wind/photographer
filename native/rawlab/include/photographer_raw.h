// SPDX-License-Identifier: GPL-3.0-only
#pragma once
#include <stddef.h>
#include <stdint.h>
#if defined(_WIN32) && defined(PHOTOGRAPHER_RAW_BUILD)
#define PC_RAW_API __declspec(dllexport)
#elif defined(_WIN32)
#define PC_RAW_API __declspec(dllimport)
#else
#define PC_RAW_API __attribute__((visibility("default")))
#endif
#ifdef __cplusplus
extern "C" {
#endif
typedef int (*pc_raw_cancel)(void *userdata);
typedef struct {
    uint32_t version;
    uint32_t half_size;
    uint64_t max_pixels;
    float white_balance[4]; // zero: as-shot; otherwise absolute R/G/B/G
    pc_raw_cancel cancelled;
    void *userdata;
} pc_raw_options;
typedef struct {
    uint32_t width, height;
    uint32_t source_width, source_height;
    uint32_t color_contract; // 1: linear sRGB/D65; 2: display-sRGB
    uint32_t gpu_used; // actual platform success, never the requested mode
    float baseline_ev;
    const float *rgb;
    size_t samples;
    void *owner;
    char error[256];
} pc_raw_result;
typedef struct {
    uint32_t version;
    const char *lut_path; // empty: neutral; core identifies color contract
    float strength; // 0..1, display-encoded mix
    uint32_t gpu_mode; // 0 off, 1 auto (CPU fallback), 2 force
    pc_raw_cancel cancelled;
    void *userdata;
} pc_film_options;
PC_RAW_API uint32_t pc_raw_abi_version(void);
// UTF-8 file source: RAWProcessor has no public buffer API at this revision.
// Source must remain stable during this synchronous call. Free previous owner
// before reusing result storage. No exception may cross this ABI.
PC_RAW_API int pc_raw_decode(const char *path, const pc_raw_options *options, pc_raw_result *result);
PC_RAW_API int pc_raw_render(const float *rgb, uint32_t width, uint32_t height, const pc_film_options *options, pc_raw_result *result);
PC_RAW_API void pc_raw_free(void *owner);
#ifdef __cplusplus
}
#endif
