#pragma once

// With clang, the standard assert won't make the address sanitizer spit
// out the stack trace.
#define ouisync_assert(x) if (!x) { volatile void* p = nullptr; *((int*) p) = 1; }
