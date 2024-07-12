#
# To learn more about a Podspec see http://guides.cocoapods.org/syntax/podspec.html.
# Run `pod lib lint ouisync.podspec` to validate before publishing.
#
Pod::Spec.new do |s|
  s.name             = 'ouisync'
  s.version          = '0.0.1'
  s.summary          = 'Ouisync flutter plugin.'
  s.description      = <<-DESC
A new Flutter plugin project.
                       DESC
  s.homepage         = 'http://example.com'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'eQualitie' => 'support@ouisync.net' }

  s.source           = { :path => '.' }
  s.source_files     = 'Classes/**/*'
  s.static_framework = true
  s.vendored_libraries = "**/*.dylib"
  s.dependency 'FlutterMacOS'

  s.platform = :osx, '10.11'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.swift_version = '5.0'
end
