#include "photographer_raw.h"
#include <cmath>
#include <cstdio>
// Release checks stay active without assert/NDEBUG dependence.
int main() {
    pc_raw_options options{}; options.version = 2; options.max_pixels = 128000000;
    pc_raw_result result{};
    if (pc_raw_abi_version() != 3 || pc_raw_decode("missing-source.DNG", &options, &result) == 0 || result.owner || result.rgb || !result.error[0]) return 1;
    options.version = 999;
    if (pc_raw_decode("missing-source.DNG", &options, &result) == 0 || result.owner || !result.error[0]) return 2;
    options.version = 2; options.cancelled = [](void *) { return 1; };
    if (pc_raw_decode("missing-source.DNG", &options, &result) == 0 || result.owner || !result.error[0]) return 3;
    const float linear[] = {0,0,0,.1845f,.1845f,.1845f,1,1,1,2,2,2};
    pc_film_options film{}; film.version = 2; film.strength = 0;
    film.relative_white_balance[0] = film.relative_white_balance[1] = film.relative_white_balance[2] = 1;
    film.lut_path = "missing-film.cube"; // zero is a bypass, no resource read
    if (pc_raw_render(linear, 2, 2, &film, &result) != 0 || !result.owner || !result.rgb || result.samples != 12 || result.color_contract != 2 || result.gpu_used) return 4;
    if (result.rgb[0] != 0 || std::abs(result.rgb[3] - .4664f) > .001f || result.rgb[9] <= result.rgb[6]) { pc_raw_free(result.owner); return 5; }
    pc_raw_free(result.owner);
    film.strength = 1;
    if (pc_raw_render(linear, 2, 2, &film, &result) == 0 || result.owner || !result.error[0]) return 6;
    film.version = 1; // ABI 1 options must never be read as ABI 2 gains/exposure.
    if (pc_raw_render(linear, 2, 2, &film, &result) == 0 || result.owner || !result.error[0]) return 7;
    pc_raw_free(nullptr);
    std::puts("RawLab float ABI: failure/cancel/neutral range/resource bypass checked");
    return 0;
}
