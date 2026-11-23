#!/bin/sh

[ -z "${IPKG_INSTROOT:-}" ] || exit 0
[ "${IPKG_NO_SCRIPT:-}" != 1 ] || exit 0

service=/etc/init.d/ouinode
[ -x "$service" ] && "$service" running || exit 0

exec "$service" restart
