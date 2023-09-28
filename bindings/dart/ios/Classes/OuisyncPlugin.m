#import "OuisyncPlugin.h"
#if __has_include(<ouisync_plugin/ouisync_plugin-Swift.h>)
#import <ouisync_plugin/ouisync_plugin-Swift.h>
#else
// Support project import fallback if the generated compatibility header
// is not copied when this plugin is created as a library.
// https://forums.swift.org/t/swift-static-libraries-dont-copy-generated-objective-c-header/19816
#import "ouisync_plugin-Swift.h"
#endif

@implementation OuisyncPlugin
+ (void)registerWithRegistrar:(NSObject<FlutterPluginRegistrar>*)registrar {
  [SwiftOuisyncPlugin registerWithRegistrar:registrar];
}
@end
