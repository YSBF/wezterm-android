import org.gradle.internal.os.OperatingSystem

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val abis: List<String> = (project.findProperty("wezterm.abis") as String)
    .split(",")
    .map { it.trim() }
    .filter { it.isNotEmpty() }

val minSdkVersion = (project.findProperty("wezterm.minSdk") as String).toInt()

/** Maps an Android ABI onto the Rust target triple that produces it. */
fun rustTargetFor(abi: String): String = when (abi) {
    "arm64-v8a" -> "aarch64-linux-android"
    "armeabi-v7a" -> "armv7-linux-androideabi"
    "x86_64" -> "x86_64-linux-android"
    "x86" -> "i686-linux-android"
    else -> throw GradleException("unknown ABI $abi")
}

android {
    namespace = "org.wezfurlong.wezterm"
    compileSdk = 35
    ndkVersion = "28.0.12674087"

    defaultConfig {
        applicationId = "org.wezfurlong.wezterm"
        minSdk = minSdkVersion
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters += abis
        }
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
            isJniDebuggable = true
        }
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    packaging {
        jniLibs {
            // Must stay true. The native library directory is the only place an
            // app may both read and execute from since API 29, so any bundled
            // shell has to be extracted there rather than left compressed
            // inside the APK. See docs/android-port-plan.md.
            useLegacyPackaging = true
        }
    }

    splits {
        abi {
            isEnable = abis.size > 1
            reset()
            include(*abis.toTypedArray())
            isUniversalApk = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets {
        getByName("main") {
            // Prebuilt shell and utility binaries, named lib<command>.so and
            // laid out per ABI. The native library directory is the only place
            // an app may both read and execute from since API 29, so this is
            // where a bundled bash or busybox has to go; the Rust side links
            // them to their real names at startup. See android/README.md.
            jniLibs.srcDir("src/main/prefix")
        }
        // The cdylib is staged per build type rather than in one shared
        // directory, so that building both variants cannot have one overwrite
        // the other's library. See the cargo tasks below.
        buildTypes.forEach { buildType ->
            getByName(buildType.name) {
                jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs/${buildType.name}"))
            }
        }
    }
}

dependencies {
    // GameActivity, not NativeActivity: it ships GameTextInput, which maps far
    // more directly onto the commit/composing-text handling a terminal needs.
    // Do NOT enable prefab support for this; android-activity supplies its own
    // C++ glue layer and the upstream one is incompatible with it.
    implementation("androidx.games:games-activity:4.4.0")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.core:core-ktx:1.15.0")
}

/**
 * Build the Rust cdylib for each ABI and stage it where the Android plugin
 * expects to find jniLibs.
 *
 * This deliberately shells out to cargo rather than using cargo-ndk, so that
 * the only tooling required is a Rust toolchain and an NDK. ci/android-env.sh
 * is the single definition of the cross-compile environment and is shared with
 * command-line builds.
 *
 * There is one task per build type rather than one shared task that infers the
 * profile from the requested task names. Inferring it is wrong twice over: a
 * task name that does not mention the variant (`build`, `assemble`) builds
 * everything but would pick the debug profile for the release APK too, and
 * asking for both variants at once would give both APKs whichever profile won.
 */
fun registerCargoBuild(buildType: String, release: Boolean) = tasks.register(
    "cargoBuild${buildType.replaceFirstChar { it.uppercase() }}",
) {
    description = "Cross-compiles the wezterm $buildType cdylib for each configured ABI"
    group = "build"

    doLast {
        if (OperatingSystem.current().isWindows) {
            throw GradleException("building the native library on Windows is not supported")
        }

        val repoRoot = project.rootDir.parentFile
        val stagingRoot = layout.buildDirectory.dir("rustJniLibs/$buildType").get().asFile

        // Debug builds of the whole GUI are enormous (hundreds of MB per ABI),
        // which is fine for `adb install` but not for anything else.
        val profileArgs = if (release) listOf("--release") else emptyList()
        val profileDir = if (release) "release" else "debug"

        abis.forEach { abi ->
            val target = rustTargetFor(abi)

            val script = buildString {
                append(". ci/android-env.sh && ")
                append("cargo build --target $target -p wezterm-android ")
                append("--no-default-features --features vendored-fonts ")
                append(profileArgs.joinToString(" "))
            }

            providers.exec {
                workingDir = repoRoot
                commandLine("bash", "-lc", script)
            }.result.get().assertNormalExitValue()

            val built = File(repoRoot, "target/$target/$profileDir/libwezterm_gui.so")
            if (!built.exists()) {
                throw GradleException("cargo did not produce ${built.path}")
            }

            val destDir = File(stagingRoot, abi)
            destDir.mkdirs()
            built.copyTo(File(destDir, "libwezterm_gui.so"), overwrite = true)

            // GameActivity's C++ glue pulls in the shared libc++ runtime, so it
            // has to travel with us; the NDK does not ship it inside the app
            // unless we copy it.
            val ndkDir = android.ndkDirectory
            val libcxx = File(
                ndkDir,
                "toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/" +
                    "${sysrootDirFor(abi)}/libc++_shared.so",
            )
            if (libcxx.exists()) {
                libcxx.copyTo(File(destDir, "libc++_shared.so"), overwrite = true)
            } else {
                logger.warn("libc++_shared.so not found at ${libcxx.path}")
            }
        }
    }
}

/** The NDK sysroot subdirectory that holds the runtime libs for an ABI. */
fun sysrootDirFor(abi: String): String = when (abi) {
    "arm64-v8a" -> "aarch64-linux-android"
    "armeabi-v7a" -> "arm-linux-androideabi"
    "x86_64" -> "x86_64-linux-android"
    "x86" -> "i686-linux-android"
    else -> throw GradleException("unknown ABI $abi")
}

android.buildTypes.forEach { buildType ->
    val name = buildType.name
    val cargo = registerCargoBuild(name, release = !buildType.isDebuggable)
    val variant = name.replaceFirstChar { it.uppercase() }
    tasks.withType<com.android.build.gradle.tasks.MergeSourceSetFolders>().configureEach {
        if (this.name == "merge${variant}JniLibFolders") {
            dependsOn(cargo)
        }
    }
}
