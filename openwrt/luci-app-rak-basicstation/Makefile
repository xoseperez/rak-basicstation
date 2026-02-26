include $(TOPDIR)/rules.mk

LUCI_TITLE:=RAK BasicStation Forwarder
LUCI_DEPENDS:=+luci-base +rak-basicstation
PKG_MAINTAINER:=RAKwireless

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
$(eval $(call BuildPackage,luci-app-rak-basicstation))
