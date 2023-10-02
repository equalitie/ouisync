#include "include/ouisync_plugin/ouisync_plugin_c_api.h"

#include <flutter/plugin_registrar_windows.h>

#include "ouisync_plugin.h"

void OuisyncPluginCApiRegisterWithRegistrar(
    FlutterDesktopPluginRegistrarRef registrar) {
  ouisync_plugin::OuisyncPlugin::RegisterWithRegistrar(
      flutter::PluginRegistrarManager::GetInstance()
          ->GetRegistrar<flutter::PluginRegistrarWindows>(registrar));
}
