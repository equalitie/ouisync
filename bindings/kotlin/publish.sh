#!/usr/bin/env bash

# Script for publishing the kotlin packages to Maven Cental.

program=$(basename "$0")
dir=$(dirname "$0")

usage() {
    echo "Usage:"
    echo ""
    echo "  $program <PACKAGE>..."
    echo "  $program --all"
    echo "  $program --help"
    echo ""
    echo "Available packages: session, releaseService, debugService, releaseAndroid, debugAndroid"
}

publish() {
    local tasks=""

    for package in $@; do
        tasks="$tasks publish${package^}PublicationToSonatypeRepository"
    done

    tasks="$tasks closeSonatypeStagingRepository"

    gradle $tasks
}

gradle() {
    cd $dir

    OSSRH_USERNAME="$(pass cenoers/ouinet/central-token | grep username | cut -d'>' -f 2 | cut -d'<' -f 1)" \
    OSSRH_PASSWORD="$(pass cenoers/ouinet/central-token | grep password | cut -d'>' -f 2 | cut -d'<' -f 1)" \
    SONATYPE_STAGING_PROFILE_ID="7874402e84339c"                                                            \
    SIGNING_PASSWORD="$(pass cenoers/ouinet/gpg-subkey-F2DDC823 | head -1)"                                 \
    SIGNING_KEY_ID=F2DDC823                                                                                 \
    SIGNING_KEY="$(pass cenoers/ouinet/gpg-subkey-F2DDC823-armor)"                                          \
    ./gradlew $@
}

case "$1" in
"" | "-h" | "--help")
    usage
    ;;
"--all")
    # Note that publishing release and debug variants in the same gradle invocation currently
    # doesn't work as it would end up bundling the release version of the native libraries into
    # both variants. This is a limitation of the Rust Android Gradle plugin we are using.
    # Publishing release and debug separately works around it.
    publish "session" "releaseService" "releaseAndroid"
    publish           "debugService"   "debugAndroid"
    ;;
*)
    publish ${@:1}
    ;;
esac
