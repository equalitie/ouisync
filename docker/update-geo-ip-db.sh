#!/bin/bash

# Update the geo-ip database used for analytics. This should be scheduled with cron (or similar) to
# run every two weeks or so.

# Add maxmind.com license key here:
LICENSE_KEY=

NAME="GeoLite2-Country"
URL="https://download.maxmind.com/app/geoip_download?edition_id=${NAME}&license_key=${LICENSE_KEY}&suffix=tar.gz"

archive="$NAME.tar.gz"
tmp_dir=$(mktemp -d)

curl --output "$tmp_dir/$archive" $URL
(cd $tmp_dir; tar -xf $archive)
rm "$tmp_dir/$archive"

src_dir=$(find $tmp_dir -name "${NAME}_*" | head -n1)
src="$src_dir/$NAME.mmdb"

docker cp $src ouisync:/config/



