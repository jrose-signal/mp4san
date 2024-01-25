//! WARNING: This is not a 100% correct implementation of a frame dumper for H.264 in MPEG-4.
//!
//! Many many things were skipped and/or hardcoded. Do not use this as a reference, only a starting
//! point.

use std::io;
use std::io::Read as _;

use mp4san::parse::MoovBox;

pub fn main() {
    env_logger::init();

    let mut input = Vec::with_capacity(100 * 1024);
    io::stdin().read_to_end(&mut input).expect("can read stdin");

    let moov = mp4san_dump::parse(&input).expect("valid input");
    let moov_children = moov.data.parsed::<MoovBox>().expect("parsed moov box already").parsed();

    // FIXME: The first track isn't always the video track.
    if let Some(track) = moov_children.tracks.get(0) {
        let vt_session = video_toolbox::VTDecompressionSession::new().expect("success");

        mp4san_dump::for_each_sample(track, &input, |sample| {
            vt_session.decode(sample).expect("can write samples");
            Ok(())
        })
        .expect("valid parsed input");
    }
}

// Everything below is about exposing macOS APIs to Rust.

mod video_toolbox {
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    use core_foundation::base::{kCFAllocatorDefault, CFAllocatorRef, CFType, CFTypeRef, OSStatus, TCFType as _};
    use core_foundation::dictionary::CFDictionaryRef;
    use core_foundation::string::CFStringRef;
    use core_foundation::url::{CFURLRef, CFURL};
    use hex_literal::hex;

    unsafe fn check_create(op: impl FnOnce(*mut CFTypeRef) -> OSStatus) -> Result<CFType, OSStatus> {
        let mut out = std::ptr::null();
        check_status(op(&mut out))?;
        Ok(CFType::wrap_under_create_rule(out))
    }

    fn check_status(status: OSStatus) -> Result<(), OSStatus> {
        match status {
            0 => Ok(()),
            error => Err(error),
        }
    }
    pub struct VTDecompressionSession {
        format_desc: CFType,
        session: CFType,
        counter: Arc<AtomicU64>,
    }

    impl VTDecompressionSession {
        pub fn new() -> Result<Self, core_foundation::base::OSStatus> {
            let hardcoded_sps = hex!("67640033 ACB401E0 021F4D40 404041E2 C5D4").as_slice();
            let hardcoded_pps = hex!("68EE0D8B").as_slice();

            let format_desc = unsafe {
                check_create(|out| {
                    CMVideoFormatDescriptionCreateFromH264ParameterSets(
                        kCFAllocatorDefault,
                        2,
                        [hardcoded_sps.as_ptr(), hardcoded_pps.as_ptr()].as_ptr(),
                        [hardcoded_sps.len(), hardcoded_pps.len()].as_ptr(),
                        4,
                        out,
                    )
                })?
            };

            let inner = unsafe {
                check_create(|out| {
                    VTDecompressionSessionCreate(
                        kCFAllocatorDefault,
                        format_desc.as_CFTypeRef(),
                        std::ptr::null(),
                        std::ptr::null(),
                        std::ptr::null(),
                        out,
                    )
                })?
            };

            Ok(Self { format_desc, session: inner, counter: Default::default() })
        }

        pub fn decode(&self, samples: &[u8]) -> Result<(), OSStatus> {
            let buffer = unsafe { check_create(|out| CMBlockBufferCreateEmpty(kCFAllocatorDefault, 0, 0, out))? };
            unsafe {
                check_status(CMBlockBufferAppendMemoryBlock(
                    buffer.as_CFTypeRef(),
                    std::ptr::null(),
                    samples.len(),
                    kCFAllocatorDefault,
                    std::ptr::null(),
                    0,
                    samples.len(),
                    0,
                ))?;
                check_status(CMBlockBufferReplaceDataBytes(
                    samples.as_ptr(),
                    buffer.as_CFTypeRef(),
                    0,
                    samples.len(),
                ))?;
            }

            let sample_buffer = unsafe {
                check_create(|out| {
                    CMSampleBufferCreateReady(
                        kCFAllocatorDefault,
                        buffer.as_CFTypeRef(),
                        self.format_desc.as_CFTypeRef(),
                        1,
                        0,
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        out,
                    )
                })?
            };

            unsafe {
                let frame_counter = self.counter.clone();
                check_status(VTDecompressionSessionDecodeFrameWithOutputHandler(
                    self.session.as_CFTypeRef(),
                    sample_buffer.as_CFTypeRef(),
                    0,
                    std::ptr::null(),
                    &*block2::ConcreteBlock::new(move |status, _flags, buffer, _timestamp, _duration| {
                        let 0 = status else {
                            log::error!("frame decode error {status}");
                            return;
                        };

                        let next_index = frame_counter.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        log::info!("writing frame {next_index}...");
                        Self::write_buffer_to_file(buffer, next_index)
                            .unwrap_or_else(|e| log::error!("failed to write to file: {e}"));
                    })
                    .copy(),
                ))
            }
        }

        fn write_buffer_to_file(buffer: CVPixelBufferRef, index: u64) -> Result<(), OSStatus> {
            let image = unsafe { check_create(|out| VTCreateCGImageFromCVPixelBuffer(buffer, std::ptr::null(), out))? };
            let url = CFURL::from_path(format!("{index}.png"), false).expect("valid relative URL");

            let destination = unsafe {
                CFType::wrap_under_create_rule(CGImageDestinationCreateWithURL(
                    url.as_concrete_TypeRef(),
                    kUTTypePNG,
                    1,
                    std::ptr::null(),
                ))
            };
            unsafe { CGImageDestinationAddImage(destination.as_CFTypeRef(), image.as_CFTypeRef(), std::ptr::null()) };
            let success = unsafe { CGImageDestinationFinalize(destination.as_CFTypeRef()) };
            if !success {
                return Err(-4960); // coreFoundationUnknownErr. Why doesn't ImageIO have error reporting?
            }
            Ok(())
        }
    }

    // Everything below *here* mirrors what's in Apple's headers.

    #[repr(C)]
    struct CMTime {
        value: i64,     /*< The value of the CMTime. value/timescale = seconds */
        timescale: i32, /*< The timescale of the CMTime. value/timescale = seconds. */
        flags: u32,     /*< The flags, eg. kCMTimeFlags_Valid, kCMTimeFlags_PositiveInfinity, etc. */
        epoch: i64,     /*< Differentiates between equal timestamps that are actually different because
                        of looping, multi-item sequencing, etc.
                        Will be used during comparison: greater epochs happen after lesser ones.
                        Additions/subtraction is only possible within a single epoch,
                        however, since epoch length may be unknown/variable */
    }

    unsafe impl objc2::encode::Encode for CMTime {
        const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
            // The name of the type that Objective-C sees.
            "CMTime",
            &[
                // Delegate to field's implementations.
                // The order is the same as in the definition.
                i64::ENCODING,
                i32::ENCODING,
                u32::ENCODING,
                i64::ENCODING,
            ],
        );
    }

    #[repr(C)]
    struct CMSampleTimingInfo {
        duration: CMTime, /*< The duration of the sample. If a single struct applies to
                          each of the samples, they all will have this duration. */
        presentation_time_stamp: CMTime, /*< The time at which the sample will be presented. If a single
                                         struct applies to each of the samples, this is the presentationTime of the
                                         first sample. The presentationTime of subsequent samples will be derived by
                                         repeatedly adding the sample duration. */
        decode_time_stamp: CMTime, /*< The time at which the sample will be decoded. If the samples
                                   are in presentation order (eg. audio samples, or video samples from a codec
                                   that doesn't support out-of-order samples), this can be set to kCMTimeInvalid. */
    }

    type CMBlockBufferRef = CFTypeRef;
    type CMSampleBufferRef = CFTypeRef;
    type CMFormatDescriptionRef = CFTypeRef;
    type CMVideoFormatDescriptionRef = CMFormatDescriptionRef;
    type CVImageBufferRef = CFTypeRef;
    type CVPixelBufferRef = CVImageBufferRef;
    type VTDecompressionSessionRef = CFTypeRef;
    type CGImageRef = CFTypeRef;
    type CGImageDestinationRef = CFTypeRef;

    type VTDecompressionOutputHandler = *const block2::Block<(OSStatus, u32, CVImageBufferRef, CMTime, CMTime), ()>;

    #[link(name = "VideoToolbox", kind = "framework")]
    extern "C" {
        fn VTDecompressionSessionCreate(
            allocator: CFAllocatorRef,
            videoFormatDescription: CMVideoFormatDescriptionRef,
            videoDecoderSpecification: CFDictionaryRef,
            destinationImageBufferAttributes: CFDictionaryRef,
            outputCallback: *const c_void,
            decompressionSessionOut: *mut VTDecompressionSessionRef,
        ) -> OSStatus;

        fn VTDecompressionSessionDecodeFrameWithOutputHandler(
            session: VTDecompressionSessionRef,
            sampleBuffer: CMSampleBufferRef,
            decodeFlags: u32,
            infoFlagsOut: *const c_void,
            outputHandler: VTDecompressionOutputHandler,
        ) -> OSStatus;

        fn VTCreateCGImageFromCVPixelBuffer(
            pixelBuffer: CVPixelBufferRef,
            options: CFDictionaryRef,
            imageOut: *mut CGImageRef,
        ) -> OSStatus;
    }

    #[link(name = "CoreMedia", kind = "framework")]
    extern "C" {
        fn CMBlockBufferCreateEmpty(
            structureAllocator: CFAllocatorRef,
            subBlockCapacity: u32,
            flags: u32,
            blockBufferOut: *mut CMBlockBufferRef,
        ) -> OSStatus;

        fn CMBlockBufferAppendMemoryBlock(
            theBuffer: CMBlockBufferRef,
            memoryBlock: *const c_void,
            blockLength: usize,
            blockAllocator: CFAllocatorRef,
            customBlockSource: *const c_void,
            offsetToData: usize,
            dataLength: usize,
            flags: u32,
        ) -> OSStatus;

        fn CMBlockBufferReplaceDataBytes(
            sourceBytes: *const u8,
            destinationBuffer: CMBlockBufferRef,
            offsetIntoDestination: usize,
            dataLength: usize,
        ) -> OSStatus;

        fn CMSampleBufferCreateReady(
            allocator: CFAllocatorRef,
            dataBuffer: CMBlockBufferRef,
            formatDescription: CMFormatDescriptionRef,
            numSamples: usize,
            numSampleTimingEntries: usize,
            sampleTimingArray: *const CMSampleTimingInfo,
            numSampleSizeEntries: usize,
            sampleSizeArray: *const usize,
            sampleBufferOut: *mut CMSampleBufferRef,
        ) -> OSStatus;

        fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
            allocator: CFAllocatorRef, /* @param allocator
                                       CFAllocator to be used when creating the CMFormatDescription. Pass NULL to use the default allocator. */
            parameterSetCount: usize, /* @param parameterSetCount
                                      The number of parameter sets to include in the format description. This parameter must be at least 2. */
            parameterSetPointers: *const *const u8, /* @param parameterSetPointers
                                                    Points to a C array containing parameterSetCount pointers to parameter sets. */
            parameterSetSizes: *const usize, /* @param parameterSetSizes
                                             Points to a C array containing the size, in bytes, of each of the parameter sets. */
            NALUnitHeaderLength: i32, /* @param NALUnitHeaderLength
                                      Size, in bytes, of the NALUnitLength field in an AVC video sample or AVC parameter set sample. Pass 1, 2 or 4. */
            formatDescriptionOut: *mut CMVideoFormatDescriptionRef,
        ) -> OSStatus;
    }

    #[link(name = "ImageIO", kind = "framework")]
    extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: CFURLRef,
            ty: CFStringRef,
            count: usize,
            options: CFDictionaryRef,
        ) -> CGImageDestinationRef;
        fn CGImageDestinationAddImage(
            destination: CGImageDestinationRef,
            image: CGImageRef,
            properties: CFDictionaryRef,
        );
        fn CGImageDestinationFinalize(destination: CGImageDestinationRef) -> bool;
    }

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        static kUTTypePNG: CFStringRef;
    }
}
