# https://github.com/java-native-access/jna/blob/master/www/FrequentlyAskedQuestions.md#jna-on-android
-dontwarn java.awt.*
-keep class com.sun.jna.* { *; }
-keepclassmembers class * extends com.sun.jna.* { public *; }

# These additional rules seem to be necessary to prevent runtime exceptions
-dontwarn sun.nio.ch.DirectBuffer
-keep class org.equalitie.ouisync.service.Bindings { *; }
-keep class org.msgpack.core.buffer.MessageBufferU { *; }
