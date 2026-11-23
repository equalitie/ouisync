.DEFAULT_GOAL := all

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/..)
ARCH ?= aarch64 armv7 mips mipsle
CROSS ?= cross
APK ?= apk
PKG_RELEASE ?= 1
PKG_VERSION ?= $(shell sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' "$(ROOT)/Cargo.toml")

triples := aarch64 armv7 mips mipsle

target.aarch64 := aarch64-unknown-linux-musl
target.armv7 := armv7-unknown-linux-musleabihf
target.mips := mips-unknown-linux-musl
target.mipsle := mipsel-unknown-linux-musl

arch.aarch64 := aarch64_generic
arch.armv7 := arm_cortex-a7_neon-vfpv4
arch.mips := mips_24kc
arch.mipsle := mipsel_24kc

selected := $(sort $(patsubst mipsel,mipsle,$(ARCH)))
ifeq ($(selected),)
  $(error Set ARCH to one or more of: $(triples))
endif

ifneq ($(filter-out $(triples),$(selected)),)
  $(error Unsupported ARCH: $(filter-out $(triples),$(selected)); expected: $(triples))
endif

ifneq ($(strip $(PKG_ARCH)),)
  ifneq ($(words $(selected)),1)
    $(error PKG_ARCH can only be set when building one architecture)
  endif
endif

formats := $(or $(sort $(filter ipk apk,$(MAKECMDGOALS))),ipk apk)
ifneq ($(filter all,$(MAKECMDGOALS)),)
  formats := ipk apk
endif

builds := $(addprefix build-,$(selected))

.PHONY: all ipk apk $(builds)
all ipk apk: $(builds)

$(builds): build-%:
	CROSS="$(CROSS)" APK="$(APK)" sh "$(ROOT)/openwrt/build-package.sh" \
		"$(target.$*)" "$(or $(PKG_ARCH),$(arch.$*))" "$(PKG_VERSION)-r$(PKG_RELEASE)" $(formats)
