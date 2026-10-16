find_package(PkgConfig REQUIRED)
pkg_check_modules(GLIB2 REQUIRED IMPORTED_TARGET glib-2.0)
file(READ "${CMAKE_CURRENT_SOURCE_DIR}/upstream/lutools/cmake/lens-dependencies.json" lens_lock)
string(JSON lens_url GET "${lens_lock}" lensfun url)
string(JSON lens_sha GET "${lens_lock}" lensfun sha256)
FetchContent_Declare(pc_lensfun URL "${lens_url}" URL_HASH "SHA256=${lens_sha}")
FetchContent_GetProperties(pc_lensfun)
if(NOT pc_lensfun_POPULATED)
    FetchContent_Populate(pc_lensfun)
endif()
# Lensfun 0.3.4 predates CMake 4. Configure its unchanged source separately.
set(lens_prefix "${CMAKE_BINARY_DIR}/lensfun-install")
execute_process(COMMAND "${CMAKE_COMMAND}" -S "${pc_lensfun_SOURCE_DIR}" -B "${CMAKE_BINARY_DIR}/lensfun-build"
    -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DCMAKE_BUILD_TYPE=Release
    "-DCMAKE_OSX_ARCHITECTURES=${CMAKE_OSX_ARCHITECTURES}" "-DCMAKE_OSX_DEPLOYMENT_TARGET=${CMAKE_OSX_DEPLOYMENT_TARGET}"
    "-DCMAKE_INSTALL_PREFIX=${lens_prefix}" -DCMAKE_POSITION_INDEPENDENT_CODE=ON
    -DBUILD_STATIC=ON -DBUILD_TESTS=OFF -DBUILD_LENSTOOL=OFF -DBUILD_DOC=OFF
    -DBUILD_FOR_SSE=OFF -DBUILD_FOR_SSE2=OFF -DPYTHON=OFF -DINSTALL_PYTHON_MODULE=OFF -DINSTALL_HELPER_SCRIPTS=OFF
    RESULT_VARIABLE lens_status)
if(lens_status EQUAL 0)
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${CMAKE_BINARY_DIR}/lensfun-build" --parallel 2 RESULT_VARIABLE lens_status)
endif()
if(lens_status EQUAL 0)
    execute_process(COMMAND "${CMAKE_COMMAND}" --install "${CMAKE_BINARY_DIR}/lensfun-build" RESULT_VARIABLE lens_status)
endif()
if(NOT lens_status EQUAL 0)
    message(FATAL_ERROR "Pinned Lensfun build failed: ${lens_status}")
endif()
file(MAKE_DIRECTORY "${lens_prefix}/lib/pkgconfig")
file(WRITE "${lens_prefix}/lib/pkgconfig/lensfun.pc"
    "prefix=${lens_prefix}\nlibdir=\${prefix}/lib\nincludedir=\${prefix}/include\nName: lensfun\nDescription: Pinned Lensfun\nVersion: 0.3.4\nRequires: glib-2.0\nLibs: -L\${libdir} -llensfun\nCflags: -I\${includedir}/lensfun -DCONF_LENSFUN_STATIC\n")
set(ENV{PKG_CONFIG_PATH} "${lens_prefix}/lib/pkgconfig:$ENV{PKG_CONFIG_PATH}")
# Re-query after prepending our pinned installation, even on reconfigure.
unset(__pkg_config_checked_LENSFUN CACHE)
unset(__pkg_config_arguments_LENSFUN CACHE)
