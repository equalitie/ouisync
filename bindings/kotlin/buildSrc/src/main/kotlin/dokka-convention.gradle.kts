plugins {
    id("org.jetbrains.dokka")
}

dokka {
    pluginsConfiguration.html {
        customStyleSheets.from(rootProject.layout.files("docs/logo-styles.css"))
        customAssets.from(rootProject.layout.files("docs/logo-icon.png"))
        footerMessage.set("""
            Copyright (c) 2024 <a class="footer--link" href="https://equalit.ie">eQualitie</a> |
            <a class="footer--link" href="https://ouisync.net">Ouisync website</a> |
            <a class="footer--link" href="https://github.com/equalitie/ouisync">Ouisync git repository</a>
            """.trimIndent())
    }
}
