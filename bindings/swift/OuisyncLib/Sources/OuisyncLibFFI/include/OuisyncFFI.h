//
//  FileProviderProxy.h
//  Runner
//
//  Created by Peter Jankuliak on 03/04/2024.
//

#ifndef OuisyncFFI_h
#define OuisyncFFI_h

#include <stdint.h>

typedef uint64_t OuisyncClientHandle;

struct OuisyncSessionCreateResult {
    OuisyncClientHandle clientHandle;
    uint16_t errorCode;
    const char* errorMessage;
};

typedef struct OuisyncSessionCreateResult OuisyncSessionCreateResult;

#endif /* OuisyncFFI_h */
