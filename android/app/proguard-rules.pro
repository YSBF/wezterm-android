# GameActivity and GameTextInput are reached from native code via JNI, so R8
# cannot see the references and would otherwise strip them.
-keep class com.google.androidgamesdk.** { *; }
-keep class androidx.games.** { *; }

# The Activity is named in the manifest and instantiated reflectively.
-keep class org.wezfurlong.wezterm.WezTermActivity { *; }
-keep class org.wezfurlong.wezterm.MuxService { *; }
