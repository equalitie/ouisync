#import <Flutter/Flutter.h>

@interface OuisyncPlugin : NSObject<FlutterPlugin>
@end

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

enum DartCObjectType {
  TypedData = 7,
};
typedef int32_t DartCObjectType;

enum DartTypedDataType {
  Uint8 = 2,
};
typedef int32_t DartTypedDataType;

enum ErrorCode {
  /**
   * No error
   */
  Ok = 0,
  /**
   * Store error
   */
  Store = 1,
  /**
   * Insuficient permission to perform the intended operation
   */
  PermissionDenied = 2,
  /**
   * Malformed data
   */
  MalformedData = 3,
  /**
   * Entry already exists
   */
  EntryExists = 4,
  /**
   * Entry doesn't exist
   */
  EntryNotFound = 5,
  /**
   * Multiple matching entries found
   */
  AmbiguousEntry = 6,
  /**
   * The intended operation requires the directory to be empty but it isn't
   */
  DirectoryNotEmpty = 7,
  /**
   * The indended operation is not supported
   */
  OperationNotSupported = 8,
  /**
   * Failed to read from or write into the config file
   */
  Config = 10,
  /**
   * Argument passed to a function is not valid
   */
  InvalidArgument = 11,
  /**
   * Request or response is malformed
   */
  MalformedMessage = 12,
  /**
   * Storage format version mismatch
   */
  StorageVersionMismatch = 13,
  /**
   * Connection lost
   */
  ConnectionLost = 14,
  VfsInvalidMountPoint = 2048,
  VfsDriverInstall = (2048 + 1),
  VfsBackend = (2048 + 2),
  /**
   * Unspecified error
   */
  Other = 65535,
};
typedef uint16_t ErrorCode;

/**
 * FFI handle to a resource with unique ownership.
 */
typedef uint64_t UniqueHandle_Session;

typedef UniqueHandle_Session SessionHandle;

typedef struct SessionCreateResult {
  SessionHandle session;
  ErrorCode error_code;
  const char *error_message;
} SessionCreateResult;

typedef void (*Callback)(void *context, const uint8_t *msg_ptr, uint64_t msg_len);

typedef int64_t Port;

typedef struct DartTypedData {
  DartTypedDataType type_;
  intptr_t length;
  uint8_t *values;
} DartTypedData;

typedef union DartCObjectValue {
  struct DartTypedData as_typed_data;
  uint64_t _align[5];
} DartCObjectValue;

typedef struct DartCObject {
  DartCObjectType type_;
  union DartCObjectValue value;
} DartCObject;

typedef bool (*PostDartCObjectFn)(Port, struct DartCObject*);

typedef uint64_t Handle_FileHolder;

/**
 * Creates a ouisync session (common C-like API)
 *
 * # Safety
 *
 * - `configs_path` and `log_path` must be pointers to nul-terminated utf-8 encoded strings.
 * - `context` must be a valid pointer to a value that outlives the `Session` and that is safe
 *   to be sent to other threads or null.
 * - `callback` must be a valid function pointer which does not leak the passed `msg_ptr`.
 */
struct SessionCreateResult session_create(const char *configs_path,
                                          const char *log_path,
                                          void *context,
                                          Callback callback);

/**
 * Creates a ouisync session (dart-specific API)
 *
 * # Safety
 *
 * - `configs_path` and `log_path` must be pointers to nul-terminated utf-8 encoded strings.
 * - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
 */
struct SessionCreateResult session_create_dart(const char *configs_path,
                                               const char *log_path,
                                               PostDartCObjectFn post_c_object_fn,
                                               Port port);

/**
 * Closes the ouisync session.
 *
 * # Safety
 *
 * `session` must be a valid session handle.
 */
void session_close(SessionHandle session);

/**
 * # Safety
 *
 * `session` must be a valid session handle, `sender` must be a valid client sender handle,
 * `payload_ptr` must be a pointer to a byte buffer whose length is at least `payload_len` bytes.
 *
 */
void session_channel_send(SessionHandle session, uint8_t *payload_ptr, uint64_t payload_len);

/**
 * Shutdowns the network and closes the session. This is equivalent to doing it in two steps
 * (`network_shutdown` then `session_close`), but in flutter when the engine is being detached
 * from Android runtime then async wait for `network_shutdown` never completes (or does so
 * randomly), and thus `session_close` is never invoked. My guess is that because the dart engine
 * is being detached we can't do any async await on the dart side anymore, and thus need to do it
 * here.
 *
 * # Safety
 *
 * `session` must be a valid session handle.
 */
void session_shutdown_network_and_close(SessionHandle session);

/**
 * Copy the file contents into the provided raw file descriptor (dart-specific API).
 *
 * This function takes ownership of the file descriptor and closes it when it finishes. If the
 * caller needs to access the descriptor afterwards (or while the function is running), he/she
 * needs to `dup` it before passing it into this function.
 *
 * # Safety
 *
 * - `session` must be a valid session handle
 * - `handle` must be a valid file holder handle
 * - `fd` must be a valid and open file descriptor
 * - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
 * - `port` must be a valid dart native port
 */
void file_copy_to_raw_fd_dart(SessionHandle session,
                              Handle_FileHolder handle,
                              int fd,
                              PostDartCObjectFn post_c_object_fn,
                              Port port);

/**
 * Always returns `OperationNotSupported` error. Defined to avoid lookup errors on non-unix
 * platforms. Do not use.
 *
 * # Safety
 *
 * - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
 * - `port` must be a valid dart native port.
 * - `session`, `handle` and `fd` are not actually used and so have no safety requirements.
 */
void file_copy_to_raw_fd_dart(SessionHandle _session,
                              Handle_FileHolder _handle,
                              int _fd,
                              PostDartCObjectFn post_c_object_fn,
                              Port port);

/**
 * Deallocate string that has been allocated on the rust side
 *
 * # Safety
 *
 * `ptr` must be a pointer obtained from a call to `CString::into_raw`.
 */
void free_string(char *ptr);

/**
 * Print log message
 *
 * # Safety
 *
 * `message_ptr` must be a pointer to a nul-terminated utf-8 encoded string
 */
void log_print(uint8_t level, const char *scope_ptr, const char *message_ptr);