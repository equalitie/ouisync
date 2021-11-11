#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define ENTRY_TYPE_INVALID 0

#define ENTRY_TYPE_FILE 1

#define ENTRY_TYPE_DIRECTORY 2

/**
 * FFI handle to a resource with shared ownership.
 */
typedef uint64_t SharedHandle_Repository;

typedef int64_t Port;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_UniqueHandle_Directory;

/**
 * FFI handle to a resource with unique ownership.
 */
typedef uint64_t UniqueHandle_Directory;

/**
 * FFI handle to a borrowed resource.
 */
typedef uint64_t RefHandle_DirEntry;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_SharedHandle_Mutex_File;

/**
 * FFI handle to a resource with shared ownership.
 */
typedef uint64_t SharedHandle_Mutex_File;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_u64;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_SharedHandle_Repository;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_u8;

/**
 * FFI handle to a resource with unique ownership.
 */
typedef uint64_t UniqueHandle_JoinHandle;

/**
 * FFI handle to a resource with unique ownership.
 */
typedef uint64_t UniqueHandle_Repository;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_bool;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_SharedHandle_Handle;

void directory_create(SharedHandle_Repository repo, const char *path, Port port, char **error);

void directory_open(SharedHandle_Repository repo,
                    const char *path,
                    Port_UniqueHandle_Directory port,
                    char **error);

/**
 * Removes the directory at the given path from the repository. The directory must be empty.
 */
void directory_remove(SharedHandle_Repository repo, const char *path, Port port, char **error);

/**
 * Removes the directory at the given path including its content from the repository.
 */
void directory_remove_recursively(SharedHandle_Repository repo,
                                  const char *path,
                                  Port port,
                                  char **error);

void directory_close(UniqueHandle_Directory handle);

uint64_t directory_num_entries(UniqueHandle_Directory handle);

RefHandle_DirEntry directory_get_entry(UniqueHandle_Directory handle, uint64_t index);

const char *dir_entry_name(RefHandle_DirEntry handle);

uint8_t dir_entry_type(RefHandle_DirEntry handle);

void file_open(SharedHandle_Repository repo,
               const char *path,
               Port_SharedHandle_Mutex_File port,
               char **error);

void file_create(SharedHandle_Repository repo,
                 const char *path,
                 Port_SharedHandle_Mutex_File port,
                 char **error);

/**
 * Remove (delete) the file at the given path from the repository.
 */
void file_remove(SharedHandle_Repository repo, const char *path, Port port, char **error);

void file_close(SharedHandle_Mutex_File handle, Port port, char **error);

void file_flush(SharedHandle_Mutex_File handle, Port port, char **error);

/**
 * Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
 * (zero on EOF).
 */
void file_read(SharedHandle_Mutex_File handle,
               uint64_t offset,
               uint8_t *buffer,
               uint64_t len,
               Port_u64 port,
               char **error);

/**
 * Write `len` bytes from `buffer` into the file.
 */
void file_write(SharedHandle_Mutex_File handle,
                uint64_t offset,
                const uint8_t *buffer,
                uint64_t len,
                Port port,
                char **error);

/**
 * Truncate the file to `len` bytes.
 */
void file_truncate(SharedHandle_Mutex_File handle, uint64_t len, Port port, char **error);

/**
 * Retrieve the size of the file in bytes.
 */
void file_len(SharedHandle_Mutex_File handle, Port_u64 port, char **error);

/**
 * Opens a repository.
 */
void repository_open(const char *store, Port_SharedHandle_Repository port, char **error);

/**
 * Closes a repository.
 */
void repository_close(SharedHandle_Repository handle);

/**
 * Returns the type of repository entry (file, directory, ...).
 * If the entry doesn't exists, returns `ENTRY_TYPE_INVALID`, not an error.
 */
void repository_entry_type(SharedHandle_Repository handle,
                           const char *path,
                           Port_u8 port,
                           char **error);

/**
 * Move/rename entry from src to dst.
 */
void repository_move_entry(SharedHandle_Repository handle,
                           const char *src,
                           const char *dst,
                           Port port,
                           char **error);

/**
 * Subscribe to change notifications from the repository.
 */
UniqueHandle_JoinHandle repository_subscribe(SharedHandle_Repository handle, Port port);

/**
 * Cancel the repository change notifications subscription.
 */
void subscription_cancel(UniqueHandle_JoinHandle handle);

void repository_is_dht_enabled(UniqueHandle_Repository handle, Port_bool port);

void repository_enable_dht(UniqueHandle_Repository handle, Port port);

void repository_disable_dht(UniqueHandle_Repository handle, Port port);

/**
 * Opens the ouisync session. `post_c_object_fn` should be a pointer to the dart's
 * `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
 * around limitations of the binding generators).
 */
void session_open(const void *post_c_object_fn, const char *store, Port port, char **error_ptr);

/**
 * Closes the ouisync session.
 */
void session_close(void);

void session_get_network(Port_SharedHandle_Handle port, char **error_ptr);
