#
# To learn more about a Podspec see http://guides.cocoapods.org/syntax/podspec.html.
# Run `pod lib lint ouisync_plugin.podspec` to validate before publishing.
#
Pod::Spec.new do |s|
  s.name             = 'ouisync_plugin'
  s.version          = '0.0.1'
  s.summary          = 'A new flutter plugin project.'
  s.description      = <<-DESC
A new flutter plugin project.
                       DESC
  s.homepage         = 'http://example.com'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'Your Company' => 'email@example.com' }
  s.source           = { :path => '.' }
  s.public_header_files = 'Classes**/*.h'
  s.source_files = 'Classes/**/*'
  s.static_framework = true
  s.vendored_libraries = "**/*.a"
  s.dependency 'Flutter'
  s.platform = :ios, '11.0'

  # Flutter.framework does not contain a i386 slice.
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => 'i386' }
  s.swift_version = '5.0'

  # Need more testing: sometimes it would not work.
  # s.script_phase = { 
  #   :name => 'Building ouisync library...',
  #   :script => 'cd "${PODS_ROOT}/../../ouisync/" && cargo lipo --release && cd "${PODS_ROOT}/../../ouisync/bindings/dart/ios/" && ln -sf "${PODS_ROOT}/../../ouisync/target/universal/release/libouisync_ffi.a" .',
  #   :execution_position => :before_compile,
  #   :output_files => ['"${PODS_ROOT}/../../ouisync/target/universal/release/libouisync_ffi.a"']
  # }
end
