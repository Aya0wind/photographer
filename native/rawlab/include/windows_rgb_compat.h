// SPDX-License-Identifier: GPL-3.0-only
#pragma once
#ifdef _WIN32
// Read SDK declarations once before core headers, then remove the function-like
// RGB macro. Later windows.h includes are guarded and cannot redefine it over
// sony2fuji::RGB constructors. NOMINMAX is supplied by the target already.
#include <windows.h>
#ifdef RGB
#undef RGB
#endif
#endif
