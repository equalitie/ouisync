#include "index.h"
#include "hash.h"
#include "ouisync_assert.h"
#include "variant.h"
#include "ouisync_assert.h"
#include "block_store.h"
#include "iterator_range.h"
#include "ostream/set.h"

#include <iostream>

using namespace ouisync;
using std::string;
using std::move;
using std::set;
using std::cerr;

Index::Index(const UserId& user_id, VersionedObject commit)
{
    _blocks[commit.id][commit.id][user_id] = 1;
    _commits[user_id] = move(commit);
}

void Index::set_version_vector(const UserId& user, const VersionVector& vv)
{
    auto ci = _commits.find(user);

    ouisync_assert(ci != _commits.end());
    if (ci == _commits.end()) exit(1);

    if (!vv.happened_after(ci->second.versions)) return;
    if (vv == ci->second.versions) return;
    ci->second.versions = vv;
}

template<class Map, class F>
static
void zip(Map& a, Map& b, F&& modifier)
{
    auto ai = a.begin();
    auto bi = b.begin();

    while (ai != a.end() || bi != b.end()) {
        if (ai == a.end()) {
            auto rni = std::next(bi);
            modifier(bi->first, ai, bi);
            bi = rni;
        }
        else if (bi == b.end()) {
            auto lni = std::next(ai);
            modifier(ai->first, ai, bi);
            ai = lni;
        }
        else if (ai->first == bi->first) {
            auto lni = std::next(ai);
            auto rni = std::next(bi);
            modifier(ai->first, ai, bi);
            ai = lni;
            bi = rni;
        } else {
            if (ai->first < bi->first) {
                auto lni = std::next(ai);
                modifier(ai->first, ai, b.end());
                ai = lni;
            } else {
                auto rni = std::next(bi);
                modifier(bi->first, a.end(), bi);
                bi = rni;
            }
        }
    }
}

struct Index::Item
{
    using Oi = BlockMap::iterator;
    using Pi = ParentMap::iterator;
    using Ui = UserMap  ::iterator;

    BlockMap& blocks;

    Opt<Oi> oi;
    Opt<Pi> pi;
    Opt<Ui> ui;

    Item(BlockMap& os)                      : blocks(os) {}
    Item(BlockMap& os, Oi oi)               : blocks(os), oi(oi)                 { normalize(); }
    Item(BlockMap& os, Oi oi, Pi pi)        : blocks(os), oi(oi), pi(pi)         { normalize(); }
    Item(BlockMap& os, Oi oi, Pi pi, Ui ui) : blocks(os), oi(oi), pi(pi), ui(ui) { normalize(); }

    // Make so that none of `oi`, `pi`, `ui` are `*.end()`s, but instead they're
    // set to boost::none.
    void normalize() {
        using boost::none;
        if (!oi)                        { oi = none; pi = none; ui = none; return; }
        if (*oi == blocks.end())        { oi = none; pi = none; ui = none; return; }
        if (*pi == (*oi)->second.end()) {            pi = none; ui = none; return; }
        if (*ui == (*pi)->second.end()) {                       ui = none; return; }
    }

    operator bool() const { return oi && pi && ui; }

    Index::Count get_count() const {
        ouisync_assert(ui);
        return (*ui)->second;
    }

    void set_count(Index::Count cnt) {
        ouisync_assert(cnt > 0);
        ouisync_assert(ui);
        (*ui)->second = cnt;
    }

    void erase()
    {
        if (ui) {
            (*pi)->second.erase(*ui);
        }
        if (pi && (*pi)->second.empty()) {
            (*oi)->second.erase(*pi);
        }
        if (oi && (*oi)->second.empty()) {
            blocks.erase(*oi);
        }
    }
};

template<class F> void Index::compare(const BlockMap& remote_blocks, F&& cmp)
{
    // Const cast below because it would have double the code if the `Item`
    // class had to work with both `::iterator`s and `::const_iterator`s.
    auto& lo = _blocks;
    auto& ro = const_cast<BlockMap&>(remote_blocks);

    zip(lo, ro,
        [&] (BlockId blk, auto blk_li, auto blk_ri) {
            if (blk_li != _blocks.end() && blk_ri != remote_blocks.end()) {
                zip(parents(blk_li),
                    parents(blk_ri),
                    [&](auto parent_id, auto parent_li, auto parent_ri)
                    {
                        if (parent_li != parents(blk_li).end() && parent_ri != parents(blk_ri).end()) {
                            zip(users(parent_li), users(parent_ri),
                                [&](auto user_id, auto user_li, auto user_ri) {
                                    cmp(blk, parent_id, user_id,
                                        Item(lo, blk_li, parent_li, user_li),
                                        Item(ro, blk_ri, parent_ri, user_ri));
                                });
                        }
                        else if (parent_ri == parents(blk_ri).end()) {
                            for (auto user_li : iterator_range(users(parent_li))) {
                                cmp(blk, parent_id, id(user_li),
                                    Item(lo, blk_li, parent_li, user_li),
                                    Item(ro, blk_ri, parent_ri));
                            }
                        }
                        else if (parent_li == parents(blk_li).end()) {
                            for (auto user_ri : iterator_range(users(parent_ri))) {
                                cmp(blk, parent_id, id(user_ri),
                                    Item(lo, blk_li, parent_li),
                                    Item(ro, blk_ri, parent_ri, user_ri));
                            }
                        }
                        else {
                            ouisync_assert(0);
                        }
                    });
            }
            else if (blk_ri == remote_blocks.end()) {
                for (auto parent_li : iterator_range(parents(blk_li))) {
                    for (auto user_li : iterator_range(users(parent_li))) {
                        cmp(blk, id(parent_li), id(user_li),
                            Item(lo, blk_li, parent_li, user_li),
                            Item(ro, blk_ri));
                    }
                }
            }
            else if (blk_li == _blocks.end()) {
                for (auto parent_ri : iterator_range(parents(blk_ri))) {
                    for (auto user_ri : iterator_range(users(parent_ri))) {
                        cmp(blk, id(parent_ri), id(user_ri),
                            Item(lo, blk_li),
                            Item(ro, blk_ri, parent_ri, user_ri));
                    }
                }
            }
            else {
                ouisync_assert(0);
            }
        });
}

void Index::merge(const Index& remote_index, BlockStore& block_store)
{
    auto& remote_commits = remote_index._commits;
    auto& remote_blocks  = remote_index._blocks;

    set<UserId> is_newer;

    for (auto& [remote_user, remote_commit] : remote_commits) {
        if (remote_is_newer(remote_commit, remote_user)) {
            is_newer.insert(remote_user);
        }
    }

    compare(remote_blocks,
        [&] (auto& block_id, auto& parent_id, auto& user_id, Item local, Item remote)
        {
            if (!is_newer.count(user_id)) return;

            if (local && remote) {
                ouisync_assert(remote.get_count() > 0);
                local.set_count(remote.get_count());
            }
            else if (local) {
                auto id = block_id;

                local.erase();

                if (!someone_has(id)) {
                    block_store.remove(id);
                    _missing_blocks.erase(id);
                }
            }
            else if (remote) {
                bool block_is_new = !someone_has(block_id);

                auto& um = _blocks[block_id][parent_id];
                auto ui = um.insert({user_id, 0}).first;
                ui->second = remote.get_count();

                ouisync_assert(ui->second > 0);

                if (block_id == parent_id) {
                    _commits[user_id] = remote_commits.at(user_id);
                }

                if (block_is_new) _missing_blocks.insert(block_id);
            }
            else {
                ouisync_assert(0);
            }
        });
}

bool Index::remote_is_newer(const VersionedObject& remote_commit, const UserId& user) const
{
    auto li = _commits.find(user);

    if (li == _commits.end()) {
        return remote_commit.versions.version_of(user) > 0;
    } else {
        // Sincle user can't/must not create concurrent versions, so comparing
        // the single version number is sufficcient.
        return remote_commit.versions.version_of(user) > li->second.versions.version_of(user);
    }
}

void Index::insert_block(const UserId& user, const BlockId& block_id, const BlockId& parent_id, size_t cnt)
{
    if (cnt == 0) return;

    auto block_i  = _blocks.insert({block_id, {}}).first;
    auto parent_i = block_i->second.insert({parent_id, {}}).first;
    auto user_i   = parent_i->second.insert({user, 0}).first;

    user_i->second += cnt;

    if (block_id == parent_id) {
        _commits[user].id = block_id;
    }
}

void Index::remove_block(const UserId& user, const BlockId& block_id, const BlockId& parent_id)
{
    auto block_i = _blocks.find(block_id);
    ouisync_assert(block_i != _blocks.end());
    if (block_i == _blocks.end()) return;

    auto parent_i = block_i->second.find(parent_id);
    ouisync_assert(parent_i != block_i->second.end());
    if (parent_i == block_i->second.end()) return;

    auto user_i = parent_i->second.find(user);
    ouisync_assert(user_i != parent_i->second.end());
    if (user_i == parent_i->second.end()) return;

    if (--user_i->second == 0) {
        Item(_blocks, block_i, parent_i, user_i).erase();
    }
}

bool Index::someone_has(const BlockId& block) const
{
    return _blocks.find(block) != _blocks.end();
}

bool Index::block_is_missing(const BlockId& block) const
{
    return _missing_blocks.find(block) != _missing_blocks.end();
}

bool Index::mark_not_missing(const BlockId& block)
{
    return _missing_blocks.erase(block) != 0;
}

Opt<VersionedObject> Index::commit(const UserId& user)
{
    auto i = _commits.find(user);
    if (i == _commits.end()) return boost::none;
    return i->second;
}

std::set<BlockId> Index::roots() const
{
    std::set<BlockId> ret;
    for (auto& [user, commit] : _commits) {
        ret.insert(commit.id);
    }
    return ret;
}

std::ostream& ouisync::operator<<(std::ostream& os, const Index& index)
{
    os << "Commits = {\n";
    for (auto& [user, commit] : index._commits) {
        os << "  User:" << user << " Root:" << commit.id << " Versions:" << commit.versions << "\n";
    }
    os << "}\n";
    os << "Blocks = {\n";
    for (auto& [block, parents]: index._blocks) {
        for (auto& [parent, users]: parents) {
            for (auto& [user, count]: users) {
                os << "  Block:" << block << " Parent:" << parent
                    << " User:" << user << " Count:" << count << "\n";
            }
        }
    }
    os << "}\n";

    os << "Missing = " << index._missing_blocks << "\n";

    return os;
}
