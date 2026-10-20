#
# To learn more about a Podspec see http://guides.cocoapods.org/syntax/podspec.html.
# Run `pod lib lint logtee.podspec` to validate before publishing.
#
Pod::Spec.new do |s|
  s.name             = 'logtee'
  s.version          = '0.0.1'
  s.summary          = 'Logtee flutter plugin.'
  s.description      = <<-DESC
Captures a process's log output to a rotated file.
                       DESC
  s.homepage         = 'http://example.com'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'eQualitie' => 'support@ouisync.net' }

  s.source           = { :path => '.' }
  s.public_header_files = 'Classes**/*.h'
  s.source_files     = 'Classes/**/*'
  s.static_framework = true
  s.ios.vendored_frameworks = "ios/Logtee.framework"
  s.osx.vendored_frameworks = "Logtee.framework"
  s.ios.dependency 'Flutter'
  s.osx.dependency 'FlutterMacOS'

  s.ios.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => 'i386' }
  s.osx.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.swift_version = '5.0'
end
