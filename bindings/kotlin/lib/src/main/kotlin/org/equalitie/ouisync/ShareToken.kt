package org.equalitie.ouisync

class ShareToken internal constructor(private val value: String, private val client: Client) {
    companion object {
        /**
         * Creates share token from a raw String
         */
        suspend fun fromString(session: Session, string: String): ShareToken {
            val client = session.client
            val value = client.invoke(ShareTokenNormalize(string)) as String

            return ShareToken(value, client)
        }
    }

    /**
     * Returns the access mode this share token grants.
     */
    suspend fun accessMode(): AccessMode {
        val raw = client.invoke(ShareTokenMode(value)) as Byte
        return AccessMode.decode(raw)
    }

    /**
     * Returns the info-hash of the repository
     */
    suspend fun infoHash() = client.invoke(ShareTokenInfoHash(value)) as String

    /**
     * Returns the suggested name for the repository
     */
    suspend fun suggestedName() = client.invoke(ShareTokenSuggestedName(value)) as String

    override operator fun equals(other: Any?) = other is ShareToken && value == other.value

    override fun toString() = value
}
