plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.denvion.splitseal"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.denvion.splitseal"
        minSdk = 24
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            // Which ABIs the app ships; must match what cargo-ndk builds into jniLibs.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    buildFeatures { compose = true }
    composeOptions { kotlinCompilerExtensionVersion = "1.5.14" }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.06.00")
    implementation(composeBom)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.4")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    // Backdrop blur for the floating bottom bar — the iOS `.ultraThinMaterial` equivalent.
    // 0.7.x is the last line built against Compose 1.6 / Kotlin 1.9; newer needs Compose 1.7+.
    // Real blur needs API 31+, below that it degrades to a translucent scrim.
    implementation("dev.chrisbanes.haze:haze:0.7.3")
    implementation("com.google.zxing:core:3.5.3") // QR code for the profile card
    implementation("com.journeyapps:zxing-android-embedded:4.3.0") // camera QR scanner
    // "Scan document" in the attach sheet: camera capture with edge detection, returning a PDF.
    // Runs in Play services, so the pages never pass through this app before they are sealed.
    implementation("com.google.android.gms:play-services-mlkit-document-scanner:16.0.0-beta1")
}
