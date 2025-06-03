# https://github.com/java-native-access/jna/blob/master/www/FrequentlyAskedQuestions.md#jna-on-android
-dontwarn java.awt.*
-keep class com.sun.jna.* { *; }
-keepclassmembers class * extends com.sun.jna.* { public *; }

# Prevent optimizing out `Bindings` (for some reason the above rules are not enough).
-keep class org.equalitie.ouisync.kotlin.server.Bindings { *; }
