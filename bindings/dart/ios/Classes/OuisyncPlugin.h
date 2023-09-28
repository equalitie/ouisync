#import <Flutter/Flutter.h>

@interface OuisyncPlugin : NSObject<FlutterPlugin>
@end

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define NETWORK_EVENT_PROTOCOL_VERSION_MISMATCH 0

#define NETWORK_EVENT_PEER_SET_CHANGE 1

#define ENTRY_TYPE_INVALID 0

#define ENTRY_TYPE_FILE 1

#define ENTRY_TYPE_DIRECTORY 2

#define ACCESS_MODE_BLIND 0

#define ACCESS_MODE_READ 1

#define ACCESS_MODE_WRITE 2

typedef enum ErrorCode {
  /**
   * No error
   */
  ok = 0,
  /**
   * Database error
   */
  db = 1,
  /**
   * Insuficient permission to perform the intended operation
   */
  permissionDenied = 2,
  /**
   * Malformed data
   */
  malformedData = 3,
  /**
   * Entry already exists
   */
  entryExists = 4,
  /**
   * Entry doesn't exist
   */
  entryNotFound = 5,
  /**
   * Multiple matching entries found
   */
  ambiguousEntry = 6,
  /**
   * The intended operation requires the directory to be empty but it isn't
   */
  directoryNotEmpty = 7,
  /**
   * The indended operation is not supported
   */
  operationNotSupported = 8,
  /**
   * Failed to read from or write into the device ID config file
   */
  deviceIdConfig = 10,
  /**
   * Unspecified error
   */
  other = 65536,
} ErrorCode;

/**
 * FFI handle to a resource with shared ownership.
 */
typedef uint64_t SharedHandle_Repository;

typedef int64_t Port;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_UniqueHandle_Directory;

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
typedef Port Port_Result_SharedHandle_Mutex_FfiFile;

/**
 * FFI handle to a resource with shared ownership.
 */
typedef uint64_t SharedHandle_Mutex_FfiFile;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_u64;

/**
 * FFI handle to a resource with unique ownership.
 */
typedef uint64_t UniqueHandle_JoinHandle;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_u8;

typedef struct Bytes {
  uint8_t *ptr;
  uint64_t len;
} Bytes;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_SharedHandle_RepositoryHolder;

/**
 * FFI handle to a resource with shared ownership.
 */
typedef uint64_t SharedHandle_RepositoryHolder;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_u8;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_String;

/**
 * Type-safe wrapper over native dart SendPort.
 */
typedef Port Port_Result_Vec_u8;

/**
 * FFI handle to a resource with unique ownership that can also be null.
 */
typedef uint64_t UniqueNullableHandle_JoinHandle;

void directory_create(SharedHandle_Repository repo, const char *path, Port_Result port);

void directory_open(SharedHandle_Repository repo,
                    const char *path,
                    Port_Result_UniqueHandle_Directory port);

/**
 * Removes the directory at the given path from the repository. The directory must be empty.
 */
void directory_remove(SharedHandle_Repository repo, const char *path, Port_Result port);

/**
 * Removes the directory at the given path including its content from the repository.
 */
void directory_remove_recursively(SharedHandle_Repository repo, const char *path, Port_Result port);

void directory_close(UniqueHandle_Directory handle);

uint64_t directory_num_entries(UniqueHandle_Directory handle);

RefHandle_DirEntry directory_get_entry(UniqueHandle_Directory handle, uint64_t index);

const char *dir_entry_name(RefHandle_DirEntry handle);

uint8_t dir_entry_type(RefHandle_DirEntry handle);

void file_open(SharedHandle_Repository repo,
               const char *path,
               Port_Result_SharedHandle_Mutex_FfiFile port);

void file_create(SharedHandle_Repository repo,
                 const char *path,
                 Port_Result_SharedHandle_Mutex_FfiFile port);

/**
 * Remove (delete) the file at the given path from the repository.
 */
void file_remove(SharedHandle_Repository repo, const char *path, Port_Result port);

void file_close(SharedHandle_Mutex_FfiFile handle, Port_Result port);

void file_flush(SharedHandle_Mutex_FfiFile handle, Port_Result port);

/**
 * Read at most `len` bytes from the file into `buffer`. Yields the number of bytes actually read
 * (zero on EOF).
 */
void file_read(SharedHandle_Mutex_FfiFile handle,
               uint64_t offset,
               uint8_t *buffer,
               uint64_t len,
               Port_Result_u64 port);

/**
 * Write `len` bytes from `buffer` into the file.
 */
void file_write(SharedHandle_Mutex_FfiFile handle,
                uint64_t offset,
                const uint8_t *buffer,
                uint64_t len,
                Port_Result port);

/**
 * Truncate the file to `len` bytes.
 */
void file_truncate(SharedHandle_Mutex_FfiFile handle, uint64_t len, Port_Result port);

/**
 * Retrieve the size of the file in bytes.
 */
void file_len(SharedHandle_Mutex_FfiFile handle, Port_Result_u64 port);

/**
 * Copy the file contents into the provided raw file descriptor.
 * This function takes ownership of the file descriptor and closes it when it finishes. If the
 * caller needs to access the descriptor afterwards (or while the function is running), he/she
 * needs to `dup` it before passing it into this function.
 */
void file_copy_to_raw_fd(SharedHandle_Mutex_FfiFile handle, int fd, Port_Result port);

/**
 * Binds the network to the specified addresses.
 * Rebinds if already bound. If any of the addresses is null, that particular protocol/family
 * combination is not bound. If all are null the network is disabled.
 * Yields `Ok` if the binding was successful, `Err` if any of the given addresses failed to
 * parse or are were of incorrect type (e.g. IPv4 instead of IpV6).
 */
void network_bind(const char *quic_v4,
                  const char *quic_v6,
                  const char *tcp_v4,
                  const char *tcp_v6,
                  Port_Result port);

/**
 * Subscribe to network event notifications.
 */
UniqueHandle_JoinHandle network_subscribe(Port_u8 port);

/**
 * Return the local TCP network endpoint as a string. The format is "<IPv4>:<PORT>". The
 * returned pointer may be null if we did not bind to a TCP IPv4 address.
 *
 * Example: "192.168.1.1:65522"
 *
 * IMPORTANT: the caller is responsible for deallocating the returned pointer.
 */
char *network_tcp_listener_local_addr_v4(void);

/**
 * Return the local TCP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
 * returned pointer pointer may be null if we did bind to a TCP IPv6 address.
 *
 * Example: "[2001:db8::1]:65522"
 *
 * IMPORTANT: the caller is responsible for deallocating the returned pointer.
 */
char *network_tcp_listener_local_addr_v6(void);

/**
 * Return the local QUIC/UDP network endpoint as a string. The format is "<IPv4>:<PORT>". The
 * returned pointer may be null if we did not bind to a QUIC/UDP IPv4 address.
 *
 * Example: "192.168.1.1:65522"
 *
 * IMPORTANT: the caller is responsible for deallocating the returned pointer.
 */
char *network_quic_listener_local_addr_v4(void);

/**
 * Return the local QUIC/UDP network endpoint as a string. The format is "<[IPv6]>:<PORT>". The
 * returned pointer may be null if we did bind to a QUIC/UDP IPv6 address.
 *
 * Example: "[2001:db8::1]:65522"
 *
 * IMPORTANT: the caller is responsible for deallocating the returned pointer.
 */
char *network_quic_listener_local_addr_v6(void);

/**
 * Add a QUIC endpoint to which which OuiSync shall attempt to connect. Upon failure or success
 * but then disconnection, the endpoint be retried until the below
 * `network_remove_user_provided_quic_peer` function with the same endpoint is called.
 *
 * The endpoint provided to this function may be an IPv4 endpoint in the format
 * "192.168.0.1:1234", or an IPv6 address in the format "[2001:db8:1]:1234".
 *
 * If the format is not parsed correctly, this function returns `false`, in all other cases it
 * returns `true`. The latter includes the case when the peer has already been added.
 */
bool network_add_user_provided_quic_peer(const char *addr);

/**
 * Remove a QUIC endpoint from the list of user provided QUIC peers (added by the above
 * `network_add_user_provided_quic_peer` function). Note that users added by other discovery
 * mechanisms are not affected by this function. Also, removing a peer will not cause
 * disconnection if the connection has already been established. But if the peers disconnected due
 * to other reasons, the connection to this `addr` shall not be reattempted after the call to this
 * function.
 *
 * The endpoint provided to this function may be an IPv4 endpoint in the format
 * "192.168.0.1:1234", or an IPv6 address in the format "[2001:db8:1]:1234".
 *
 * If the format is not parsed correctly, this function returns `false`, in all other cases it
 * returns `true`. The latter includes the case when the peer has not been previously added.
 */
bool network_remove_user_provided_quic_peer(const char *addr);

/**
 * Return the list of peers with which we're connected, serialized with msgpack.
 */
struct Bytes network_connected_peers(void);

/**
 * Returns our runtime id formatted as a hex string.
 * The caller is responsible for deallocating it.
 */
const char *network_this_runtime_id(void);

/**
 * Return our currently used protocol version number.
 */
uint32_t network_current_protocol_version(void);

/**
 * Return the highest seen protocol version number. The value returned is always higher
 * or equal to the value returned from network_current_protocol_version() fn.
 */
uint32_t network_highest_seen_protocol_version(void);

/**
 * Enables port forwarding (UPnP)
 */
void network_enable_port_forwarding(void);

/**
 * Disables port forwarding (UPnP)
 */
void network_disable_port_forwarding(void);

/**
 * Checks whether port forwarding (UPnP) is enabled
 */
bool network_is_port_forwarding_enabled(void);

/**
 * Enables local discovery
 */
void network_enable_local_discovery(void);

/**
 * Disables local discovery
 */
void network_disable_local_discovery(void);

/**
 * Checks whether local discovery is enabled
 */
bool network_is_local_discovery_enabled(void);

/**
 * Creates a new repository.
 */
void repository_create(const char *store,
                       const char *master_password,
                       const char *share_token,
                       Port_Result_SharedHandle_RepositoryHolder port);

/**
 * Opens an existing repository.
 */
void repository_open(const char *store,
                     const char *master_password,
                     Port_Result_SharedHandle_RepositoryHolder port);

/**
 * Closes a repository.
 */
void repository_close(SharedHandle_RepositoryHolder handle, Port port);

/**
 * Return the RepositoryId of the repository in the low hex format.
 * User is responsible for deallocating the returned string.
 */
const char *repository_low_hex_id(SharedHandle_RepositoryHolder handle);

/**
 * Return the info-hash of the repository formatted as hex string. This can be used as a globally
 * unique, non-secret identifier of the repository.
 * User is responsible for deallocating the returned string.
 */
const char *repository_info_hash(SharedHandle_RepositoryHolder handle);

/**
 * Returns the type of repository entry (file, directory, ...).
 * If the entry doesn't exists, returns `ENTRY_TYPE_INVALID`, not an error.
 */
void repository_entry_type(SharedHandle_RepositoryHolder handle,
                           const char *path,
                           Port_Result_u8 port);

/**
 * Move/rename entry from src to dst.
 */
void repository_move_entry(SharedHandle_RepositoryHolder handle,
                           const char *src,
                           const char *dst,
                           Port_Result port);

/**
 * Subscribe to change notifications from the repository.
 */
UniqueHandle_JoinHandle repository_subscribe(SharedHandle_RepositoryHolder handle, Port port);

bool repository_is_dht_enabled(SharedHandle_RepositoryHolder handle);

void repository_enable_dht(SharedHandle_RepositoryHolder handle);

void repository_disable_dht(SharedHandle_RepositoryHolder handle);

bool repository_is_pex_enabled(SharedHandle_RepositoryHolder handle);

void repository_enable_pex(SharedHandle_RepositoryHolder handle);

void repository_disable_pex(SharedHandle_RepositoryHolder handle);

void repository_create_share_token(SharedHandle_RepositoryHolder handle,
                                   uint8_t access_mode,
                                   const char *name,
                                   Port_Result_String port);

uint8_t repository_access_mode(SharedHandle_RepositoryHolder handle);

/**
 * Returns the syncing progress as a float in the 0.0 - 1.0 range.
 */
void repository_sync_progress(SharedHandle_RepositoryHolder handle, Port_Result_Vec_u8 port);

/**
 * Returns the access mode of the given share token.
 */
uint8_t share_token_mode(const char *token);

/**
 * Return the RepositoryId of the repository corresponding to the share token in the low hex format.
 * User is responsible for deallocating the returned string.
 */
const char *share_token_repository_low_hex_id(const char *token);

/**
 * Returns the info-hash of the repository corresponding to the share token formatted as hex
 * string.
 * User is responsible for deallocating the returned string.
 */
const char *share_token_info_hash(const char *token);

/**
 * IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
 */
const char *share_token_suggested_name(const char *token);

/**
 * IMPORTANT: the caller is responsible for deallocating the returned buffer unless it is `null`.
 */
struct Bytes share_token_encode(const char *token);

/**
 * IMPORTANT: the caller is responsible for deallocating the returned pointer unless it is `null`.
 */
const char *share_token_decode(const uint8_t *bytes, uint64_t len);

/**
 * Opens the ouisync session. `post_c_object_fn` should be a pointer to the dart's
 * `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
 * around limitations of the binding generators).
 */
void session_open(const void *post_c_object_fn, const char *configs_path, Port_Result port);

/**
 * Retrieve a serialized state monitor corresponding to the `path`.
 */
struct Bytes session_get_state_monitor(const uint8_t *path, uint64_t path_len);

/**
 * Subscribe to "on change" events happening inside a monitor corresponding to the `path`.
 */
UniqueNullableHandle_JoinHandle session_state_monitor_subscribe(const uint8_t *path,
                                                                uint64_t path_len,
                                                                Port port);

/**
 * Unsubscribe from the above "on change" StateMonitor events.
 */
void session_state_monitor_unsubscribe(UniqueNullableHandle_JoinHandle handle);

/**
 * Closes the ouisync session.
 */
void session_close(void);

/**
 * Cancel a notification subscription.
 */
void subscription_cancel(UniqueHandle_JoinHandle handle);

/**
 * Deallocate string that has been allocated on the rust side
 */
void free_string(char *ptr);

/**
 * Deallocate Bytes that has been allocated on the rust side
 */
void free_bytes(struct Bytes bytes);
