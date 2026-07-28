// Objective-C bridging header — exposes the Rust seal-core C ABI to Swift.
// In Xcode: Build Settings ▸ "Objective-C Bridging Header" = this file's path.
// Copy ../../seal-ffi/include/seal_ffi.h into the project (or add its dir to the
// header search paths) so this import resolves.
#import "seal_ffi.h"
