#ifndef FLUTTER_PLUGIN_OUISYNC_PLUGIN_H_
#define FLUTTER_PLUGIN_OUISYNC_PLUGIN_H_

#include <flutter/method_channel.h>
#include <flutter/plugin_registrar_windows.h>

#include <memory>

namespace ouisync_plugin {

class OuisyncPlugin : public flutter::Plugin {
 public:
  static void RegisterWithRegistrar(flutter::PluginRegistrarWindows *registrar);

  OuisyncPlugin();

  virtual ~OuisyncPlugin();

  // Disallow copy and assign.
  OuisyncPlugin(const OuisyncPlugin&) = delete;
  OuisyncPlugin& operator=(const OuisyncPlugin&) = delete;

 private:
  // Called when a method is called on this plugin's channel from Dart.
  void HandleMethodCall(
      const flutter::MethodCall<flutter::EncodableValue> &method_call,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);
};

}  // namespace ouisync_plugin

#endif  // FLUTTER_PLUGIN_OUISYNC_PLUGIN_H_
