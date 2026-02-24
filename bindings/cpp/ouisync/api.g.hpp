// This file is auto generated. Do not edit.

#include <ouisync/data.hpp>
#include <ouisync/client.hpp>
#include <boost/asio/spawn.hpp>
#include <boost/filesystem/path.hpp>

namespace ouisync {

class File {
private:
    friend class Repository;
    friend class Session;
    friend class RepositorySubscription;
    template<class, class> friend struct detail::ConvertResponse;

    std::shared_ptr<Client> client;
    FileHandle handle;

    explicit File(std::shared_ptr<Client> client, FileHandle handle) :
        client(std::move(client)),
        handle(std::move(handle))
    {}

public:
    File() {}

public:
    /**
     * Closes the file.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto close(
        CompletionToken completion_token
    ) {
        auto request = Request::FileClose{
            handle,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Flushes any pending writes to the file.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto flush(
        CompletionToken completion_token
    ) {
        auto request = Request::FileFlush{
            handle,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the length of the file in bytes
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<uint64_t>::type> CompletionToken
    >
    auto get_length(
        CompletionToken completion_token
    ) {
        auto request = Request::FileGetLength{
            handle,
        };
        return client->invoke<Response::U64, uint64_t>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the sync progress of this file, that is, the total byte size of all the blocks of
     * this file that's already been downloaded.
     *
     * Note that Ouisync downloads the blocks in random order, so until the file's been completely
     * downloaded, the already downloaded blocks are not guaranteed to continuous (there might be
     * gaps).
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<uint64_t>::type> CompletionToken
    >
    auto get_progress(
        CompletionToken completion_token
    ) {
        auto request = Request::FileGetProgress{
            handle,
        };
        return client->invoke<Response::U64, uint64_t>(std::move(request), std::move(completion_token));
    }

    /**
     * Reads `size` bytes from the file starting at `offset` bytes from the beginning of the file.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<uint8_t>>::type> CompletionToken
    >
    auto read(
        uint64_t offset,
        uint64_t size,
        CompletionToken completion_token
    ) {
        auto request = Request::FileRead{
            handle,
            offset,
            size,
        };
        return client->invoke<Response::Bytes, std::vector<uint8_t>>(std::move(request), std::move(completion_token));
    }

    /**
     * Truncates the file to the given length.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto truncate(
        uint64_t len,
        CompletionToken completion_token
    ) {
        auto request = Request::FileTruncate{
            handle,
            len,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Writes the data to the file at the given offset.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto write(
        uint64_t offset,
        const std::vector<uint8_t>& data,
        CompletionToken completion_token
    ) {
        auto request = Request::FileWrite{
            handle,
            offset,
            data,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

};

class Repository {
private:
    friend class File;
    friend class Session;
    friend class RepositorySubscription;
    template<class, class> friend struct detail::ConvertResponse;

    std::shared_ptr<Client> client;
    RepositoryHandle handle;

    explicit Repository(std::shared_ptr<Client> client, RepositoryHandle handle) :
        client(std::move(client)),
        handle(std::move(handle))
    {}

public:
    Repository() {}

public:
    /**
     * Closes the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto close(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryClose{
            handle,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Creates a new directory at the given path in the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto create_directory(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryCreateDirectory{
            handle,
            path,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Creates a new file at the given path in the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<File>::type> CompletionToken
    >
    auto create_file(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryCreateFile{
            handle,
            path,
        };
        return client->invoke<Response::File, File>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto create_mirror(
        const std::string& host,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryCreateMirror{
            handle,
            host,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Delete the repository
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto delete_repository(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryDelete{
            handle,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Deletes mirror of this repository from the given cache server host.
     *
     * Requires the repository to be opened in write mode.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto delete_mirror(
        const std::string& host,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryDeleteMirror{
            handle,
            host,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Export repository to file
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto export_repository(
        const std::string& output_path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryExport{
            handle,
            output_path,
        };
        return client->invoke<Response::Path, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto file_exists(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryFileExists{
            handle,
            path,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the access mode (*blind*, *read* or *write*) the repository is currently opened in.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<AccessMode>::type> CompletionToken
    >
    auto get_access_mode(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetAccessMode{
            handle,
        };
        return client->invoke<Response::AccessMode, AccessMode>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::chrono::milliseconds>>::type> CompletionToken
    >
    auto get_block_expiration(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetBlockExpiration{
            handle,
        };
        return client->invoke<Response::Duration, std::optional<std::chrono::milliseconds>>(std::move(request), std::move(completion_token));
    }

    /**
     * Gets the current credentials of this repository. Can be used to restore access after closing
     * and reopening the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<uint8_t>>::type> CompletionToken
    >
    auto get_credentials(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetCredentials{
            handle,
        };
        return client->invoke<Response::Bytes, std::vector<uint8_t>>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
     * exist.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<EntryType>>::type> CompletionToken
    >
    auto get_entry_type(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetEntryType{
            handle,
            path,
        };
        return client->invoke<Response::EntryType, std::optional<EntryType>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::chrono::milliseconds>>::type> CompletionToken
    >
    auto get_expiration(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetExpiration{
            handle,
        };
        return client->invoke<Response::Duration, std::optional<std::chrono::milliseconds>>(std::move(request), std::move(completion_token));
    }

    /**
     * Return the info-hash of the repository formatted as hex string. This can be used as a
     * globally unique, non-secret identifier of the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto get_info_hash(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetInfoHash{
            handle,
        };
        return client->invoke<Response::String, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_metadata(
        const std::string& key,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetMetadata{
            handle,
            key,
        };
        return client->invoke<Response::String, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_mount_point(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetMountPoint{
            handle,
        };
        return client->invoke<Response::Path, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto get_path(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetPath{
            handle,
        };
        return client->invoke<Response::Path, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<QuotaInfo>::type> CompletionToken
    >
    auto get_quota(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetQuota{
            handle,
        };
        return client->invoke<Response::QuotaInfo, QuotaInfo>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto get_short_name(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetShortName{
            handle,
        };
        return client->invoke<Response::String, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Stats>::type> CompletionToken
    >
    auto get_stats(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetStats{
            handle,
        };
        return client->invoke<Response::Stats, Stats>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the synchronization progress of this repository as the number of bytes already
     * synced ([Progress.value]) vs. the total size of the repository in bytes ([Progress.total]).
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Progress>::type> CompletionToken
    >
    auto get_sync_progress(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryGetSyncProgress{
            handle,
        };
        return client->invoke<Response::Progress, Progress>(std::move(request), std::move(completion_token));
    }

    /**
     * Is Bittorrent DHT enabled?
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_dht_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryIsDhtEnabled{
            handle,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Is Peer Exchange enabled?
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_pex_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryIsPexEnabled{
            handle,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns whether syncing with other replicas is enabled for this repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_sync_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryIsSyncEnabled{
            handle,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Checks if this repository is mirrored on the given cache server host.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto mirror_exists(
        const std::string& host,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryMirrorExists{
            handle,
            host,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto mount(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryMount{
            handle,
        };
        return client->invoke<Response::Path, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto move(
        const std::string& dst,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryMove{
            handle,
            dst,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Moves an entry (file or directory) from `src` to `dst`.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto move_entry(
        const std::string& src,
        const std::string& dst,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryMoveEntry{
            handle,
            src,
            dst,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Opens an existing file at the given path in the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<File>::type> CompletionToken
    >
    auto open_file(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryOpenFile{
            handle,
            path,
        };
        return client->invoke<Response::File, File>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the entries of the directory at the given path in the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<DirectoryEntry>>::type> CompletionToken
    >
    auto read_directory(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryReadDirectory{
            handle,
            path,
        };
        return client->invoke<Response::DirectoryEntries, std::vector<DirectoryEntry>>(std::move(request), std::move(completion_token));
    }

    /**
     * Removes the directory at the given path from the repository. If `recursive` is true it removes
     * also the contents, otherwise the directory must be empty.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto remove_directory(
        const std::string& path,
        bool recursive,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryRemoveDirectory{
            handle,
            path,
            recursive,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Removes (deletes) the file at the given path from the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto remove_file(
        const std::string& path,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryRemoveFile{
            handle,
            path,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto reset_access(
        const ShareToken& token,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryResetAccess{
            handle,
            token,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_access(
        const std::optional<AccessChange>& read,
        const std::optional<AccessChange>& write,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetAccess{
            handle,
            read,
            write,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Switches the repository to the given access mode.
     *
     * - `access_mode` is the desired access mode to switch to.
     * - `local_secret` is the local secret protecting the desired access mode. Can be `None` if no
     *   local secret is used.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_access_mode(
        const AccessMode& access_mode,
        const std::optional<LocalSecret>& local_secret,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetAccessMode{
            handle,
            access_mode,
            local_secret,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_block_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetBlockExpiration{
            handle,
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Sets the current credentials of the repository.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_credentials(
        const std::vector<uint8_t>& credentials,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetCredentials{
            handle,
            credentials,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Enables/disabled Bittorrent DHT (for peer discovery).
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_dht_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetDhtEnabled{
            handle,
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetExpiration{
            handle,
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto set_metadata(
        const std::vector<MetadataEdit>& edits,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetMetadata{
            handle,
            edits,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Enables/disables Peer Exchange (for peer discovery).
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_pex_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetPexEnabled{
            handle,
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_quota(
        const std::optional<StorageSize>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetQuota{
            handle,
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Enabled or disables syncing with other replicas.
     *
     * Note syncing is initially disabled.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_sync_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositorySetSyncEnabled{
            handle,
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<ShareToken>::type> CompletionToken
    >
    auto share(
        const AccessMode& access_mode,
        const std::optional<LocalSecret>& local_secret,
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryShare{
            handle,
            access_mode,
            local_secret,
        };
        return client->invoke<Response::ShareToken, ShareToken>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto unmount(
        CompletionToken completion_token
    ) {
        auto request = Request::RepositoryUnmount{
            handle,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

};

class Session {
private:
    friend class File;
    friend class Repository;
    friend class RepositorySubscription;
    template<class, class> friend struct detail::ConvertResponse;

    std::shared_ptr<Client> client;

    explicit Session(std::shared_ptr<Client> client) :
        client(std::move(client))
    {}

public:
    Session() {}

public:
    /**
     * Connect to the Ouisync service and return Session on success. Throws on failure.
     *
     * config_dir_path: Path to a directory created by Ouisync service
     *                  containing local_endpoint.conf file.
     */
    static Session connect(
        const boost::filesystem::path& config_dir,
        boost::asio::yield_context yield
    ) {
        auto client = ouisync::Client::connect(config_dir, yield);
        return Session(std::make_shared<Client>(std::move(client)));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto add_user_provided_peers(
        const std::vector<std::string>& addrs,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionAddUserProvidedPeers{
            addrs,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto bind_metrics(
        const std::optional<std::string>& addr,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionBindMetrics{
            addr,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto bind_network(
        const std::vector<std::string>& addrs,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionBindNetwork{
            addrs,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<uint16_t>::type> CompletionToken
    >
    auto bind_remote_control(
        const std::optional<std::string>& addr,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionBindRemoteControl{
            addr,
        };
        return client->invoke<Response::U16, uint16_t>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto copy(
        const std::optional<std::string>& src_repo,
        const std::string& src_path,
        const std::optional<std::string>& dst_repo,
        const std::string& dst_path,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionCopy{
            src_repo,
            src_path,
            dst_repo,
            dst_path,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Repository>::type> CompletionToken
    >
    auto create_repository(
        const std::string& path,
        const std::optional<SetLocalSecret>& read_secret,
        const std::optional<SetLocalSecret>& write_secret,
        const std::optional<ShareToken>& token,
        bool sync_enabled,
        bool dht_enabled,
        bool pex_enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionCreateRepository{
            path,
            read_secret,
            write_secret,
            token,
            sync_enabled,
            dht_enabled,
            pex_enabled,
        };
        return client->invoke<Response::Repository, Repository>(std::move(request), std::move(completion_token));
    }

    /**
     * Delete a repository with the given name.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto delete_repository_by_name(
        const std::string& name,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionDeleteRepositoryByName{
            name,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<SecretKey>::type> CompletionToken
    >
    auto derive_secret_key(
        const Password& password,
        const PasswordSalt& salt,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionDeriveSecretKey{
            password,
            salt,
        };
        return client->invoke<Response::SecretKey, SecretKey>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Repository>::type> CompletionToken
    >
    auto find_repository(
        const std::string& name,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionFindRepository{
            name,
        };
        return client->invoke<Response::Repository, Repository>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<PasswordSalt>::type> CompletionToken
    >
    auto generate_password_salt(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGeneratePasswordSalt();
        return client->invoke<Response::PasswordSalt, PasswordSalt>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<SecretKey>::type> CompletionToken
    >
    auto generate_secret_key(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGenerateSecretKey();
        return client->invoke<Response::SecretKey, SecretKey>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns our Ouisync protocol version.
     *
     * In order to establish connections with peers, they must use the same protocol version as
     * us.
     *
     * See also [Self::session_get_highest_seen_protocol_version]
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<uint64_t>::type> CompletionToken
    >
    auto get_current_protocol_version(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetCurrentProtocolVersion();
        return client->invoke<Response::U64, uint64_t>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::chrono::milliseconds>>::type> CompletionToken
    >
    auto get_default_block_expiration(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetDefaultBlockExpiration();
        return client->invoke<Response::Duration, std::optional<std::chrono::milliseconds>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<StorageSize>>::type> CompletionToken
    >
    auto get_default_quota(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetDefaultQuota();
        return client->invoke<Response::StorageSize, std::optional<StorageSize>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::chrono::milliseconds>>::type> CompletionToken
    >
    auto get_default_repository_expiration(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetDefaultRepositoryExpiration();
        return client->invoke<Response::Duration, std::optional<std::chrono::milliseconds>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_external_addr_v4(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetExternalAddrV4();
        return client->invoke<Response::SocketAddr, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_external_addr_v6(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetExternalAddrV6();
        return client->invoke<Response::SocketAddr, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the highest protocol version of all known peers.
     *
     * If this is higher than [our version](Self::session_get_current_protocol_version) it likely
     * means we are using an outdated version of Ouisync. When a peer with higher protocol version
     * is found, a [NetworkEvent::ProtocolVersionMismatch] is emitted.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<uint64_t>::type> CompletionToken
    >
    auto get_highest_seen_protocol_version(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetHighestSeenProtocolVersion();
        return client->invoke<Response::U64, uint64_t>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the listener addresses of this Ouisync instance.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<std::string>>::type> CompletionToken
    >
    auto get_local_listener_addrs(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetLocalListenerAddrs();
        return client->invoke<Response::PeerAddrs, std::vector<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_metrics_listener_addr(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetMetricsListenerAddr();
        return client->invoke<Response::SocketAddr, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_mount_root(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetMountRoot();
        return client->invoke<Response::Path, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<NatBehavior>>::type> CompletionToken
    >
    auto get_nat_behavior(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetNatBehavior();
        return client->invoke<Response::NatBehavior, std::optional<NatBehavior>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Stats>::type> CompletionToken
    >
    auto get_network_stats(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetNetworkStats();
        return client->invoke<Response::Stats, Stats>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns info about all known peers (both discovered and explicitly added).
     *
     * When the set of known peers changes, a [NetworkEvent::PeerSetChange] is emitted. Calling
     * this function afterwards returns the new peer info.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<PeerInfo>>::type> CompletionToken
    >
    auto get_peers(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetPeers();
        return client->invoke<Response::PeerInfos, std::vector<PeerInfo>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<std::string>>::type> CompletionToken
    >
    auto get_remote_control_listener_addr(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetRemoteControlListenerAddr();
        return client->invoke<Response::SocketAddr, std::optional<std::string>>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the listener addresses of the specified remote Ouisync instance. Works only if the
     * remote control API is enabled on the remote instance. Typically used with cache servers.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<std::string>>::type> CompletionToken
    >
    auto get_remote_listener_addrs(
        const std::string& host,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetRemoteListenerAddrs{
            host,
        };
        return client->invoke<Response::PeerAddrs, std::vector<std::string>>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the runtime id of this Ouisync instance.
     *
     * The runtime id is a unique identifier of this instance which is randomly generated every
     * time Ouisync starts.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<PublicRuntimeId>::type> CompletionToken
    >
    auto get_runtime_id(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetRuntimeId();
        return client->invoke<Response::PublicRuntimeId, PublicRuntimeId>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the access mode that the given token grants.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<AccessMode>::type> CompletionToken
    >
    auto get_share_token_access_mode(
        const ShareToken& token,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetShareTokenAccessMode{
            token,
        };
        return client->invoke<Response::AccessMode, AccessMode>(std::move(request), std::move(completion_token));
    }

    /**
     * Return the info-hash of the repository corresponding to the given token, formatted as hex
     * string.
     *
     * See also: [repository_get_info_hash]
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto get_share_token_info_hash(
        const ShareToken& token,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetShareTokenInfoHash{
            token,
        };
        return client->invoke<Response::String, std::string>(std::move(request), std::move(completion_token));
    }

    /**
     * Returns the suggested name for the repository corresponding to the given token.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::string>::type> CompletionToken
    >
    auto get_share_token_suggested_name(
        const ShareToken& token,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetShareTokenSuggestedName{
            token,
        };
        return client->invoke<Response::String, std::string>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::optional<StateMonitorNode>>::type> CompletionToken
    >
    auto get_state_monitor(
        const std::vector<MonitorId>& path,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetStateMonitor{
            path,
        };
        return client->invoke<Response::StateMonitor, std::optional<StateMonitorNode>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<std::string>>::type> CompletionToken
    >
    auto get_store_dirs(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetStoreDirs();
        return client->invoke<Response::Paths, std::vector<std::string>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::vector<std::string>>::type> CompletionToken
    >
    auto get_user_provided_peers(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionGetUserProvidedPeers();
        return client->invoke<Response::PeerAddrs, std::vector<std::string>>(std::move(request), std::move(completion_token));
    }

    /**
     * Initializes the network according to the stored configuration. If a particular network
     * parameter is not yet configured, falls back to the given defaults.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto init_network(
        const NetworkDefaults& defaults,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionInitNetwork{
            defaults,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto insert_store_dirs(
        const std::vector<std::string>& paths,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionInsertStoreDirs{
            paths,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Is local discovery enabled?
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_local_discovery_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionIsLocalDiscoveryEnabled();
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Checks whether accepting peers discovered on the peer exchange is enabled.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_pex_recv_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionIsPexRecvEnabled();
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_pex_send_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionIsPexSendEnabled();
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    /**
     * Is port forwarding (UPnP) enabled?
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto is_port_forwarding_enabled(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionIsPortForwardingEnabled();
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<std::map<std::string, Repository>>::type> CompletionToken
    >
    auto list_repositories(
        CompletionToken completion_token
    ) {
        auto request = Request::SessionListRepositories();
        return client->invoke<Response::Repositories, std::map<std::string, Repository>>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<bool>::type> CompletionToken
    >
    auto mirror_exists(
        const ShareToken& token,
        const std::string& host,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionMirrorExists{
            token,
            host,
        };
        return client->invoke<Response::Bool, bool>(std::move(request), std::move(completion_token));
    }

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
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<Repository>::type> CompletionToken
    >
    auto open_repository(
        const std::string& path,
        const std::optional<LocalSecret>& local_secret,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionOpenRepository{
            path,
            local_secret,
        };
        return client->invoke<Response::Repository, Repository>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto remove_store_dirs(
        const std::vector<std::string>& paths,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionRemoveStoreDirs{
            paths,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Removes peers previously added with [Self::session_add_user_provided_peers].
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto remove_user_provided_peers(
        const std::vector<std::string>& addrs,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionRemoveUserProvidedPeers{
            addrs,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_default_block_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetDefaultBlockExpiration{
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_default_quota(
        const std::optional<StorageSize>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetDefaultQuota{
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_default_repository_expiration(
        const std::optional<std::chrono::milliseconds>& value,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetDefaultRepositoryExpiration{
            value,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Enables/disables local discovery.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_local_discovery_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetLocalDiscoveryEnabled{
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_mount_root(
        const std::optional<std::string>& path,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetMountRoot{
            path,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_pex_recv_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetPexRecvEnabled{
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_pex_send_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetPexSendEnabled{
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Enables/disables port forwarding (UPnP).
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_port_forwarding_enabled(
        bool enabled,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetPortForwardingEnabled{
            enabled,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<void>::type> CompletionToken
    >
    auto set_store_dirs(
        const std::vector<std::string>& paths,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionSetStoreDirs{
            paths,
        };
        return client->invoke<Response::None, void>(std::move(request), std::move(completion_token));
    }

    /**
     * Checks whether the given string is a valid share token.
     */
    template<
        boost::asio::completion_token_for<typename detail::InvokeSig<ShareToken>::type> CompletionToken
    >
    auto validate_share_token(
        const std::string& token,
        CompletionToken completion_token
    ) {
        auto request = Request::SessionValidateShareToken{
            token,
        };
        return client->invoke<Response::ShareToken, ShareToken>(std::move(request), std::move(completion_token));
    }

};

} // namespace ouisync
