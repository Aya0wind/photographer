// SPDX-License-Identifier: GPL-3.0-only
#pragma once
#ifdef _WIN32
// Read SDK declarations once before core headers, then remove the function-like
// RGB macro. Later windows.h includes are guarded and cannot redefine it over
// sony2fuji::RGB constructors. NOMINMAX is supplied by the target already.
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
// LibRaw uses Winsock 2. It must precede windows.h so the SDK cannot bring in
// Winsock 1 declarations and collide with LibRaw's later winsock2.h include.
#include <winsock2.h>
#include <windows.h>
#ifdef RGB
#undef RGB
#endif
#endif
