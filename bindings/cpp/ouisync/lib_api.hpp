#pragma once

#if defined(_MSC_VER) || defined(__BORLANDC__) || defined(__CODEGEARC__) || defined(__MINGW32__)
#   define OUISYNC_USES_API
#endif

#ifdef OUISYNC_USES_API
#    if defined(OUISYNC_API_CLIENT_LOCAL)
#        define OUISYNC_CLIENT_API
#    elif defined(OUISYNC_API_CLIENT_EXPORT)
#        define OUISYNC_CLIENT_API __declspec(dllexport)
#    else
#        define OUISYNC_CLIENT_API __declspec(dllimport)
#    endif
#else
#    define OUISYNC_CLIENT_API
#endif

#ifdef OUISYNC_USES_API
#    if defined(OUISYNC_API_SERVICE_LOCAL)
#        define OUISYNC_SERVICE_API
#    elif defined(OUISYNC_API_SERVICE_EXPORT)
#        define OUISYNC_SERVICE_API __declspec(dllexport)
#    else
#        define OUISYNC_SERVICE_API __declspec(dllimport)
#    endif
#else
#    define OUISYNC_SERVICE_API
#endif

#ifdef OUISYNC_USES_API
#    if defined(OUISYNC_API_CLIENT_LOCAL) || defined(OUISYNC_API_SERVICE_LOCAL)
#        define OUISYNC_COMMON_API
#    elif defined(OUISYNC_API_CLIENT_EXPORT) || defined(OUISYNC_API_SERVICE_EXPORT)
#        define OUISYNC_COMMON_API __declspec(dllexport)
#    else
#        define OUISYNC_COMMON_API __declspec(dllimport)
#    endif
#else
#    define OUISYNC_COMMON_API
#endif
