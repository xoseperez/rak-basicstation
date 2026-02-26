include $(TOPDIR)/rules.mk

PKG_NAME:=rak-basicstation
PKG_VERSION:=0.3.0
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/xoseperez/rak-basicstation/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=707518a40e02f673db1ee72b27978cc74b63953947f520b1c57af3550a4902ba

PKG_MAINTAINER:=Xose Pérez <xose.perez@rakwireless.com>
PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE

PKG_BUILD_DEPENDS:=rust/host protobuf/host

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

export CXX:=$(TARGET_CXX_NOCACHE)
CARGO_VARS += PROTOC=$(STAGING_DIR_HOSTPKG)/bin/protoc

define Package/rak-basicstation
	SECTION:=rak
	CATEGORY:=RAK
	TITLE:=RAK BasicStation Forwarder
	URL:=https://github.com/xoseperez/rak-basicstation
	DEPENDS:=+libstdcpp
endef

define Package/rak-basicstation/description
	LoRa Basics Station protocol forwarder.
endef

define Package/rak-basicstation/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/* $(1)/usr/bin

	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/rak-basicstation.init $(1)/etc/init.d/rak-basicstation

	$(INSTALL_DIR) $(1)/lib/functions
	$(INSTALL_BIN) ./files/rak-basicstation.sh $(1)/lib/functions

	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) ./files/rak-basicstation.config $(1)/etc/config/rak-basicstation
endef

define Build/Prepare
	$(PKG_UNPACK)
	(cd $(PKG_BUILD_DIR) && rm -rf .cargo)
	$(Build/Patch)
endef

define Build/Compile
	$(call Build/Compile/Cargo)
endef

$(eval $(call RustBinPackage,rak-basicstation))
$(eval $(call BuildPackage,rak-basicstation))
