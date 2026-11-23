#!/bin/sh

[ -z "${IPKG_INSTROOT:-}" ] || exit 0
[ "${PKG_UPGRADE:-}" != 1 ] && [ "${1:-}" != upgrade ] || exit 0
[ -x /etc/init.d/ouinode ] || exit 0

/etc/init.d/ouinode stop
/etc/init.d/ouinode disable
