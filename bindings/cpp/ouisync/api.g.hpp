// This file is auto generated. Do not edit.

#include <ouisync/data.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync {

class Client;

class File {
private:
    friend class Repository;
    friend class Session;
    friend class RepositorySubscription;

    std::shared_ptr<Client> client;
    FileHandle handle;

    explicit File(std::shared_ptr<Client> client, FileHandle handle) :
        client(std::move(client)),
        handle(std::move(handle))
    {}

public:
    /**
     * Closes the file.
     */
    void close(
        boost::asio::yield_context yield
    );

    /**
     * Flushes any pending writes to the file.
     */
    void flush(
        boost::asio::yield_context yield
    );

    /**
     * Returns the length of the file in bytes
     */
    uint64_t get_length(
        boost::asio::yield_context yield
    );

    /**
     * Returns the sync progress of this file, that is, the total byte size of all the blocks of
     * this file that's already been downloaded.
     *
     * Note that Ouisync downloads the blocks in random order, so until the file's been completely
     * downloaded, the already downloaded blocks are not guaranteed to continuous (there might be
     * gaps).
     */
    uint64_t get_progress(
        boost::asio::yield_context yield
    );

    /**
     * Reads `size` bytes from the file starting at `offset` bytes from the beginning of the file.
     */
    std::vector<uint8_t> read(
        uint64_t offset,
        uint64_t size,
        boost::asio::yield_context yield
    );

    /**
     * Truncates the file to the given length.
     */
    void truncate(
        uint64_t len,
        boost::asio::yield_context yield
    );

    /**
     * Writes the data to the file at the given offset.
     */
    void write(
        uint64_t offset,
        const std::vector<uint8_t>& data,
        boost::asio::yield_context yield
    );

};

class Repository {
private:
    friend class File;
    friend class Session;
    friend class RepositorySubscription;

    std::shared_ptr<Client> client;
    RepositoryHandle handle;

    explicit Repository(std::shared_ptr<Client> client, RepositoryHandle handle) :
        client(std::move(client)),
        handle(std::move(handle))
    {}

public:
    /**
     * Closes the repository.
     */
    void close(
        boost::asio::yield_context yield
    );

    /**
     * Creates a new directory at the given path in the repository.
     */
    void create_directory(
        const std::string& path,
        boost::asio::yield_context yield
    );

    /**
     * Creates a new file at the given path in the repository.
     */
    File create_file(
        const std::string& path,
        boost::asio::yield_context yield
    );

    /**
     * Creates mirror of this repository on the given cache server host.
     *
     * Cache servers relay traffic between Ouisync peers and also temporarily store data. They are
     * useful when direct P2P connection fails (e.g. due to restrictive NAT) and also to allow
     * syncing when the peers are not online at the same time (they still need to be online within
     * ~24 hours of each other).
     *
     * Requires the repository to be opened in write mode.
     */
    void create_mirror(
        const std::string& host,
        boost::asio::yield_context yield
    );

    /**
     * Delete the repository
     */
    void delete_repository(
        boost::asio::yield_context yield
    );

    /**
     * Deletes mirror of this repository from the given cache server host.
     *
     * Requires the repository to be opened in write mode.
     */
    void delete_mirror(
        const std::string& host,
        boost::asio::yield_context yield
    );

    /**
     * Export repository to file
     */
    std::string export_repository(
        const std::string& output_path,
        boost::asio::yield_context yield
    );

    bool file_exists(
        const std::string& path,
        boost::asio::yield_context yield
    );

    /**
     * Returns the access mode (*blind*, *read* or *write*) the repository is currently opened in.
     */
    AccessMode get_access_mode(
        boost::asio::yield_context yield
    );

    std::optional<std::chrono::milliseconds> get_block_expiration(
        boost::asio::yield_context yield
    );

    /**
     * Gets the current credentials of this repository. Can be used to restore access after closing
     * and reopening the repository.
     */
    std::vector<uint8_t> get_credentials(
        boost::asio::yield_context yield
    );

    /**
     * Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
     * exist.
     */
    std::optional<EntryType> get_entry_type(
        const std::string& path,
        boost::asio::yield_context yield
    );

    std::optional<std::chrono::milliseconds> get_expiration(
        boost::asio::yield_context yield
    );

    /**
     * Return the info-hash of the repository formatted as hex string. This can be used as a
     * globally unique, non-secret identifier of the repository.
     */
    std::string get_info_hash(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_metadata(
        const std::string& key,
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_mount_point(
        boost::asio::yield_context yield
    );

    std::string get_path(
        boost::asio::yield_context yield
    );

    QuotaInfo get_quota(
        boost::asio::yield_context yield
    );

    std::string get_short_name(
        boost::asio::yield_context yield
    );

    Stats get_stats(
        boost::asio::yield_context yield
    );

    /**
     * Returns the synchronization progress of this repository as the number of bytes already
     * synced ([Progress.value]) vs. the total size of the repository in bytes ([Progress.total]).
     */
    Progress get_sync_progress(
        boost::asio::yield_context yield
    );

    /**
     * Is Bittorrent DHT enabled?
     */
    bool is_dht_enabled(
        boost::asio::yield_context yield
    );

    /**
     * Is Peer Exchange enabled?
     */
    bool is_pex_enabled(
        boost::asio::yield_context yield
    );

    /**
     * Returns whether syncing with other replicas is enabled for this repository.
     */
    bool is_sync_enabled(
        boost::asio::yield_context yield
    );

    /**
     * Checks if this repository is mirrored on the given cache server host.
     */
    bool mirror_exists(
        const std::string& host,
        boost::asio::yield_context yield
    );

    std::string mount(
        boost::asio::yield_context yield
    );

    void move(
        const std::string& dst,
        boost::asio::yield_context yield
    );

    /**
     * Moves an entry (file or directory) from `src` to `dst`.
     */
    void move_entry(
        const std::string& src,
        const std::string& dst,
        boost::asio::yield_context yield
    );

    /**
     * Opens an existing file at the given path in the repository.
     */
    File open_file(
        const std::string& path,
        boost::asio::yield_context yield
    );

    /**
     * Returns the entries of the directory at the given path in the repository.
     */
    std::vector<DirectoryEntry> read_directory(
        const std::string& path,
        boost::asio::yield_context yield
    );

    /**
     * Removes the directory at the given path from the repository. If `recursive` is true it removes
     * also the contents, otherwise the directory must be empty.
     */
    void remove_directory(
        const std::string& path,
        bool recursive,
        boost::asio::yield_context yield
    );

    /**
     * Removes (deletes) the file at the given path from the repository.
     */
    void remove_file(
        const std::string& path,
        boost::asio::yield_context yield
    );

    void reset_access(
        const ShareToken& token,
        boost::asio::yield_context yield
    );

    /**
     * Sets, unsets or changes local secrets for accessing the repository or disables the given
     * access mode.
     *
     * ## Examples
     *
     * To protect both read and write access with the same password:
     *
     * ```kotlin
     * val password = Password("supersecret")
     * repo.setAccess(read: AccessChange.Enable(password), write: AccessChange.Enable(password))
     * ```
     *
     * To require password only for writing:
     *
     * ```kotlin
     * repo.setAccess(read: AccessChange.Enable(null), write: AccessChange.Enable(password))
     * ```
     *
     * To competelly disable write access but leave read access as it was. Warning: this operation
     * is currently irreversibe.
     *
     * ```kotlin
     * repo.setAccess(read: null, write: AccessChange.Disable)
     * ```
     */
    void set_access(
        const std::optional<AccessChange>& read,
        const std::optional<AccessChange>& write,
        boost::asio::yield_context yield
    );

    /**
     * Switches the repository to the given access mode.
     *
     * - `access_mode` is the desired access mode to switch to.
     * - `local_secret` is the local secret protecting the desired access mode. Can be `None` if no
     *   local secret is used.
     */
    void set_access_mode(
        const AccessMode& access_mode,
        const std::optional<LocalSecret>& local_secret,
        boost::asio::yield_context yield
    );

    void set_block_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        boost::asio::yield_context yield
    );

    /**
     * Sets the current credentials of the repository.
     */
    void set_credentials(
        const std::vector<uint8_t>& credentials,
        boost::asio::yield_context yield
    );

    /**
     * Enables/disabled Bittorrent DHT (for peer discovery).
     */
    void set_dht_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    void set_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        boost::asio::yield_context yield
    );

    bool set_metadata(
        const std::vector<MetadataEdit>& edits,
        boost::asio::yield_context yield
    );

    /**
     * Enables/disables Peer Exchange (for peer discovery).
     */
    void set_pex_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    void set_quota(
        const std::optional<StorageSize>& value,
        boost::asio::yield_context yield
    );

    /**
     * Enabled or disables syncing with other replicas.
     *
     * Note syncing is initially disabled.
     */
    void set_sync_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    /**
     * Creates a *share token* to share this repository with other devices.
     *
     * By default the access mode of the token will be the same as the mode the repo is currently
     * opened in but it can be escalated with the `local_secret` param or de-escalated with the
     * `access_mode` param.
     *
     * - `access_mode`: access mode of the token. Useful to de-escalate the access mode to below of
     *   what the repo is opened in.
     * - `local_secret`: the local repo secret. If not `None`, the share token's access mode will
     *   be the same as what the secret provides. Useful to escalate the access mode to above of
     *   what the repo is opened in.
     */
    ShareToken share(
        const AccessMode& access_mode,
        const std::optional<LocalSecret>& local_secret,
        boost::asio::yield_context yield
    );

    void unmount(
        boost::asio::yield_context yield
    );

};

class Session {
private:
    friend class File;
    friend class Repository;
    friend class RepositorySubscription;

    std::shared_ptr<Client> client;

    explicit Session(std::shared_ptr<Client> client) :
        client(std::move(client))
    {}

public:
    /**
     *Connect to the Ouisync service and return Session on success. Throws on failure.
     *
     *config_dir_path: Path to a directory created by Ouisync service
     *                 containing local_endpoint.conf file.
     */
    static Session connect(
        const boost::filesystem::path& config_dir_path,
        boost::asio::yield_context
    );

    /**
     * Adds peers to connect to.
     *
     * Normally peers are discovered automatically (using Bittorrent DHT, Peer exchange or Local
     * discovery) but this function is useful in case when the discovery is not available for any
     * reason (e.g. in an isolated network).
     *
     * Note that peers added with this function are remembered across restarts. To forget peers,
     * use [Self::session_remove_user_provided_peers].
     */
    void add_user_provided_peers(
        const std::vector<std::string>& addrs,
        boost::asio::yield_context yield
    );

    void bind_metrics(
        const std::optional<std::string>& addr,
        boost::asio::yield_context yield
    );

    /**
     * Binds the network listeners to the specified interfaces.
     *
     * Up to four listeners can be bound, one for each combination of protocol (TCP or QUIC) and IP
     * family (IPv4 or IPv6). The format of the interfaces is "PROTO/IP:PORT" where PROTO is "tcp"
     * or "quic". If IP is IPv6, it needs to be enclosed in square brackets.
     *
     * If port is `0`, binds to a random port initially but on subsequent starts tries to use the
     * same port (unless it's already taken). This can be useful to configuring port forwarding.
     */
    void bind_network(
        const std::vector<std::string>& addrs,
        boost::asio::yield_context yield
    );

    uint16_t bind_remote_control(
        const std::optional<std::string>& addr,
        boost::asio::yield_context yield
    );

    /**
     * Copy file or directory into, from or between repositories
     *
     * - `src_repo`: Name of the repository from which file will be copied.
     * - `src_path`: Path of to the entry to be copied. If `src_repo` is set, the `src_path` is
     *   relative to the corresponding repository root. If `src_repo` is null, `src_path` is
     *   interpreted as path on the local file system.
     * - `dst_repo`: Name of the repository into which the entry will be copied.
     * - `dst_path`: Destination entry
     */
    void copy(
        const std::optional<std::string>& src_repo,
        const std::string& src_path,
        const std::optional<std::string>& dst_repo,
        const std::string& dst_path,
        boost::asio::yield_context yield
    );

    /**
     * Creates a new repository.
     *
     * - `path`: path to the repository file or name of the repository.
     * - `read_secret`: local secret for reading the repository on this device only. Do not share
     *   with peers!. If null, the repo won't be protected and anyone with physical access to the
     *   device will be able to read it.
     * - `write_secret`: local secret for writing to the repository on this device only. Do not
     *   share with peers! Can be the same as `read_secret` if one wants to use only one secret
     *   for both reading and writing.  Separate secrets are useful for plausible deniability. If
     *   both `read_secret` and `write_secret` are `None`, the repo won't be protected and anyone
     *   with physical access to the device will be able to read and write to it. If `read_secret`
     *   is not `None` but `write_secret` is `None`, the repo won't be writable from this device.
     * - `token`: used to share repositories between devices. If not `None`, this repo will be
     *   linked with the repos with the same token on other devices. See also
     *   [Self::repository_share]. This also determines the maximal access mode the repo can be
     *   opened in. If `None`, it's *write* mode.
     */
    Repository create_repository(
        const std::string& path,
        const std::optional<SetLocalSecret>& read_secret,
        const std::optional<SetLocalSecret>& write_secret,
        const std::optional<ShareToken>& token,
        bool sync_enabled,
        bool dht_enabled,
        bool pex_enabled,
        boost::asio::yield_context yield
    );

    /**
     * Delete a repository with the given name.
     */
    void delete_repository_by_name(
        const std::string& name,
        boost::asio::yield_context yield
    );

    SecretKey derive_secret_key(
        const Password& password,
        const PasswordSalt& salt,
        boost::asio::yield_context yield
    );

    Repository find_repository(
        const std::string& name,
        boost::asio::yield_context yield
    );

    PasswordSalt generate_password_salt(
        boost::asio::yield_context yield
    );

    SecretKey generate_secret_key(
        boost::asio::yield_context yield
    );

    /**
     * Returns our Ouisync protocol version.
     *
     * In order to establish connections with peers, they must use the same protocol version as
     * us.
     *
     * See also [Self::session_get_highest_seen_protocol_version]
     */
    uint64_t get_current_protocol_version(
        boost::asio::yield_context yield
    );

    std::optional<std::chrono::milliseconds> get_default_block_expiration(
        boost::asio::yield_context yield
    );

    std::optional<StorageSize> get_default_quota(
        boost::asio::yield_context yield
    );

    std::optional<std::chrono::milliseconds> get_default_repository_expiration(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_external_addr_v4(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_external_addr_v6(
        boost::asio::yield_context yield
    );

    /**
     * Returns the highest protocol version of all known peers.
     *
     * If this is higher than [our version](Self::session_get_current_protocol_version) it likely
     * means we are using an outdated version of Ouisync. When a peer with higher protocol version
     * is found, a [NetworkEvent::ProtocolVersionMismatch] is emitted.
     */
    uint64_t get_highest_seen_protocol_version(
        boost::asio::yield_context yield
    );

    /**
     * Returns the listener addresses of this Ouisync instance.
     */
    std::vector<std::string> get_local_listener_addrs(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_metrics_listener_addr(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_mount_root(
        boost::asio::yield_context yield
    );

    std::optional<NatBehavior> get_nat_behavior(
        boost::asio::yield_context yield
    );

    Stats get_network_stats(
        boost::asio::yield_context yield
    );

    /**
     * Returns info about all known peers (both discovered and explicitly added).
     *
     * When the set of known peers changes, a [NetworkEvent::PeerSetChange] is emitted. Calling
     * this function afterwards returns the new peer info.
     */
    std::vector<PeerInfo> get_peers(
        boost::asio::yield_context yield
    );

    std::optional<std::string> get_remote_control_listener_addr(
        boost::asio::yield_context yield
    );

    /**
     * Returns the listener addresses of the specified remote Ouisync instance. Works only if the
     * remote control API is enabled on the remote instance. Typically used with cache servers.
     */
    std::vector<std::string> get_remote_listener_addrs(
        const std::string& host,
        boost::asio::yield_context yield
    );

    /**
     * Returns the runtime id of this Ouisync instance.
     *
     * The runtime id is a unique identifier of this instance which is randomly generated every
     * time Ouisync starts.
     */
    PublicRuntimeId get_runtime_id(
        boost::asio::yield_context yield
    );

    /**
     * Returns the access mode that the given token grants.
     */
    AccessMode get_share_token_access_mode(
        const ShareToken& token,
        boost::asio::yield_context yield
    );

    /**
     * Return the info-hash of the repository corresponding to the given token, formatted as hex
     * string.
     *
     * See also: [repository_get_info_hash]
     */
    std::string get_share_token_info_hash(
        const ShareToken& token,
        boost::asio::yield_context yield
    );

    /**
     * Returns the suggested name for the repository corresponding to the given token.
     */
    std::string get_share_token_suggested_name(
        const ShareToken& token,
        boost::asio::yield_context yield
    );

    std::optional<StateMonitorNode> get_state_monitor(
        const std::vector<MonitorId>& path,
        boost::asio::yield_context yield
    );

    std::vector<std::string> get_store_dirs(
        boost::asio::yield_context yield
    );

    std::vector<std::string> get_user_provided_peers(
        boost::asio::yield_context yield
    );

    /**
     * Initializes the network according to the stored configuration. If a particular network
     * parameter is not yet configured, falls back to the given defaults.
     */
    void init_network(
        const NetworkDefaults& defaults,
        boost::asio::yield_context yield
    );

    void insert_store_dirs(
        const std::vector<std::string>& paths,
        boost::asio::yield_context yield
    );

    /**
     * Is local discovery enabled?
     */
    bool is_local_discovery_enabled(
        boost::asio::yield_context yield
    );

    /**
     * Checks whether accepting peers discovered on the peer exchange is enabled.
     */
    bool is_pex_recv_enabled(
        boost::asio::yield_context yield
    );

    bool is_pex_send_enabled(
        boost::asio::yield_context yield
    );

    /**
     * Is port forwarding (UPnP) enabled?
     */
    bool is_port_forwarding_enabled(
        boost::asio::yield_context yield
    );

    std::map<std::string, Repository> list_repositories(
        boost::asio::yield_context yield
    );

    bool mirror_exists(
        const ShareToken& token,
        const std::string& host,
        boost::asio::yield_context yield
    );

    /**
     * Opens an existing repository.
     *
     * - `path`: path to the local file the repo is stored in.
     * - `local_secret`: a local secret. See the `read_secret` and `write_secret` params in
     *   [Self::session_create_repository] for more details. If this repo uses local secret
     *   (s), this determines the access mode the repo is opened in: `read_secret` opens it
     *   in *read* mode, `write_secret` opens it in *write* mode and no secret or wrong secret
     *   opens it in *blind* mode. If this repo doesn't use local secret(s), the repo is opened in
     *   the maximal mode specified when the repo was created.
     */
    Repository open_repository(
        const std::string& path,
        const std::optional<LocalSecret>& local_secret,
        boost::asio::yield_context yield
    );

    void remove_store_dirs(
        const std::vector<std::string>& paths,
        boost::asio::yield_context yield
    );

    /**
     * Removes peers previously added with [Self::session_add_user_provided_peers].
     */
    void remove_user_provided_peers(
        const std::vector<std::string>& addrs,
        boost::asio::yield_context yield
    );

    void set_default_block_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        boost::asio::yield_context yield
    );

    void set_default_quota(
        const std::optional<StorageSize>& value,
        boost::asio::yield_context yield
    );

    void set_default_repository_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        boost::asio::yield_context yield
    );

    /**
     * Enables/disables local discovery.
     */
    void set_local_discovery_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    void set_mount_root(
        const std::optional<std::string>& path,
        boost::asio::yield_context yield
    );

    void set_pex_recv_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    void set_pex_send_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    /**
     * Enables/disables port forwarding (UPnP).
     */
    void set_port_forwarding_enabled(
        bool enabled,
        boost::asio::yield_context yield
    );

    void set_store_dirs(
        const std::vector<std::string>& paths,
        boost::asio::yield_context yield
    );

    /**
     * Checks whether the given string is a valid share token.
     */
    ShareToken validate_share_token(
        const std::string& token,
        boost::asio::yield_context yield
    );

};

} // namespace ouisync
