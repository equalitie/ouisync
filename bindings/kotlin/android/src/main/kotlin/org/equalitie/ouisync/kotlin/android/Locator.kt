package org.equalitie.ouisync.kotlin.android

// Structured identifier of a document in OuisyncProvider. Can represent either the list of
// repositories, a single repository or a path to a file or directory in a repository.
internal data class Locator(val repo: String, val path: String) {
    companion object {
        fun parse(documentId: String?): Locator {
            if (documentId == null || documentId == ROOT_DOCUMENT_ID) {
                return ROOT
            }

            val index = documentId.indexOf('/')
            require(index >= 0) { "invalid document id" }

            return Locator(
                repo = documentId.substring(0, index),
                path = documentId.substring(index + 1),
            )
        }

        val ROOT = Locator("", "")
        val ROOT_DOCUMENT_ID = "repos"
    }

    override fun toString() = if (isRoot()) ROOT_DOCUMENT_ID else "$repo/$path"

    val name: String
        get() = if (path.isEmpty()) repo else path.substringAfterLast('/')

    fun join(name: String): Locator = when {
        isRoot() -> Locator(repo = name, path = "")
        path.isEmpty() -> Locator(repo = repo, path = name)
        else -> Locator(repo = repo, path = "$path/$name")
    }

    val parent: Locator
        get() =
            when {
                path.isEmpty() -> ROOT
                else -> Locator(repo = repo, path = path.substringBeforeLast('/', ""))
            }

    fun isChildOf(other: Locator): Boolean {
        if (isRoot()) return false
        if (other.isRoot()) return true
        if (repo != other.repo) return false
        if (path.length <= other.path.length) return false
        if (other.path.isEmpty()) return true
        if (!path.startsWith(other.path)) return false
        if (path.get(other.path.length) != '/') return false

        return true
    }

    fun isRoot() = repo.isEmpty()
}
