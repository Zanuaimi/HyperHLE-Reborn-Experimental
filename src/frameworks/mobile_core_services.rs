/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `MobileCoreServices.framework/MobileCoreServices`.
//!
//! On iOS, MobileCoreServices is the umbrella for Uniform Type Identifier
//! types and constants (`UTType*`, `kUTType*`). It is pulled in transitively
//! by all sorts of high-level UIKit APIs — `UIDocumentInteractionController`,
//! `MFMailComposeViewController`, `UIImagePickerController` — so most apps
//! end up listing it in their Mach-O dependency table even when they never
//! call any UTType function directly.
//!
//! Without a [crate::dyld::HostDylib] entry for the path
//! `/System/Library/Frameworks/MobileCoreServices.framework/`
//! `MobileCoreServices`,
//! touchHLE prints a `Warning: app binary depends on unimplemented or missing
//! dylib …` at startup, which can spook users into reporting otherwise-fine
//! apps as broken (e.g. HyperHLE appdb report #23, Mutant Fridge).
//!
//! This implementation covers common UTType lookups and conformance checks.
//! Unknown tags and identifiers remain unsupported rather than being guessed.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::foundation::ns_string::{self, to_rust_string};
use crate::objc::nil;
use crate::Environment;

fn known_type(uti: &str) -> Option<(&'static str, &'static str)> {
    match uti {
        "public.jpeg" => Some(("jpg", "image/jpeg")),
        "public.png" => Some(("png", "image/png")),
        "com.compuserve.gif" => Some(("gif", "image/gif")),
        "public.tiff" => Some(("tif", "image/tiff")),
        "com.microsoft.bmp" => Some(("bmp", "image/bmp")),
        "com.apple.quicktime-movie" => Some(("mov", "video/quicktime")),
        "public.mpeg-4" => Some(("mp4", "video/mp4")),
        "public.mp3" => Some(("mp3", "audio/mpeg")),
        "public.mpeg-4-audio" => Some(("m4a", "audio/mp4")),
        "public.aiff-audio" => Some(("aiff", "audio/aiff")),
        "com.microsoft.waveform-audio" => Some(("wav", "audio/wav")),
        "public.plain-text" => Some(("txt", "text/plain")),
        "public.html" => Some(("html", "text/html")),
        "public.xml" => Some(("xml", "application/xml")),
        "public.json" => Some(("json", "application/json")),
        "com.adobe.pdf" => Some(("pdf", "application/pdf")),
        "public.rtf" => Some(("rtf", "application/rtf")),
        _ => None,
    }
}

fn is_known_type(uti: &str) -> bool {
    uti == "public.mpeg4"
        || CONSTANTS.iter().any(|(symbol, constant)| {
            symbol.starts_with("_kUTType")
                && !matches!(*symbol, "_kUTTypeTagSpecificationKey")
                && matches!(constant, HostConstant::NSString(value) if *value == uti)
        })
}

fn type_for_tag(tag_class: &str, tag: &str) -> Option<&'static str> {
    match tag_class {
        "public.filename-extension" => {
            match tag.trim_start_matches('.').to_ascii_lowercase().as_str() {
                "jpg" | "jpeg" => Some("public.jpeg"),
                "png" => Some("public.png"),
                "gif" => Some("com.compuserve.gif"),
                "tif" | "tiff" => Some("public.tiff"),
                "bmp" => Some("com.microsoft.bmp"),
                "mov" => Some("com.apple.quicktime-movie"),
                "mp4" => Some("public.mpeg-4"),
                "mp3" => Some("public.mp3"),
                "m4a" => Some("public.mpeg-4-audio"),
                "aiff" | "aif" => Some("public.aiff-audio"),
                "wav" => Some("com.microsoft.waveform-audio"),
                "txt" => Some("public.plain-text"),
                "html" | "htm" => Some("public.html"),
                "xml" => Some("public.xml"),
                "json" => Some("public.json"),
                "pdf" => Some("com.adobe.pdf"),
                "rtf" => Some("public.rtf"),
                _ => None,
            }
        }
        "public.mime-type" => match tag.to_ascii_lowercase().as_str() {
            "image/jpeg" => Some("public.jpeg"),
            "image/png" => Some("public.png"),
            "image/gif" => Some("com.compuserve.gif"),
            "image/tiff" => Some("public.tiff"),
            "image/bmp" => Some("com.microsoft.bmp"),
            "video/quicktime" => Some("com.apple.quicktime-movie"),
            "video/mp4" => Some("public.mpeg-4"),
            "audio/mpeg" => Some("public.mp3"),
            "audio/mp4" => Some("public.mpeg-4-audio"),
            "audio/aiff" => Some("public.aiff-audio"),
            "audio/wav" | "audio/x-wav" => Some("com.microsoft.waveform-audio"),
            "text/plain" => Some("public.plain-text"),
            "text/html" => Some("public.html"),
            "application/xml" | "text/xml" => Some("public.xml"),
            "application/json" => Some("public.json"),
            "application/pdf" => Some("com.adobe.pdf"),
            "application/rtf" => Some("public.rtf"),
            _ => None,
        },
        _ => None,
    }
}

// Direct parent relationships from Apple's system-declared UTI references.
// Types without a listed parent have no modeled ancestry.
// https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/UTIRef/Articles/System-DeclaredUniformTypeIdentifiers.html
fn direct_parents(uti: &str) -> &'static [&'static str] {
    match uti {
        "public.data" => &["public.item"],
        "public.image" | "public.text" | "public.audiovisual-content" => {
            &["public.content", "public.data"]
        }
        "public.url" => &["public.data"],
        "public.jpeg"
        | "public.png"
        | "com.compuserve.gif"
        | "public.tiff"
        | "com.microsoft.bmp"
        | "com.microsoft.ico"
        | "public.jpeg-2000"
        | "com.apple.icns"
        | "com.apple.quicktime-image" => &["public.image"],
        "public.movie" => &["public.audiovisual-content"],
        "public.video" => &["public.movie"],
        "com.apple.quicktime-movie" | "public.mpeg" | "public.mpeg-4" | "public.avi" => {
            &["public.movie"]
        }
        "public.audio" => &["public.audiovisual-content"],
        "public.mp3"
        | "com.apple.protected-mpeg-4-audio"
        | "com.microsoft.waveform-audio"
        | "public.au-audio"
        | "public.midi-audio" => &["public.audio"],
        "public.mpeg-4-audio" => &["public.audio", "public.mpeg4"],
        "public.aiff-audio" | "public.aifc-audio" => &["public.audio"],
        "public.plain-text" | "public.html" | "public.xml" | "public.json" | "public.rtf" => {
            &["public.text"]
        }
        "public.source-code" => &["public.plain-text"],
        "public.utf8-plain-text" | "public.utf16-plain-text" => &["public.plain-text"],
        "public.c-source" | "public.objective-c-source" => &["public.source-code"],
        "public.vcard" => &["public.data", "public.content"],
        "public.composite-content" => &["public.content"],
        "com.adobe.pdf" => &["public.data", "public.composite-content"],
        "public.directory" => &["public.item"],
        "public.folder" | "com.apple.bundle" => &["public.directory"],
        "com.apple.framework" => &["com.apple.bundle"],
        "public.executable" => &[],
        "com.apple.application" => &["public.executable"],
        "public.archive" => &[],
        "com.apple.property-list" => &["public.data"],
        "com.apple.xml-property-list" => &["public.xml", "com.apple.property-list"],
        "com.apple.binary-property-list" => &["com.apple.property-list"],
        _ => &[],
    }
}

fn type_conforms_to(uti: &str, parent: &str) -> bool {
    uti == parent
        || direct_parents(uti)
            .iter()
            .any(|ancestor| type_conforms_to(ancestor, parent))
}

fn UTTypeCreatePreferredIdentifierForTag(
    env: &mut Environment,
    tag_class: CFStringRef,
    tag: CFStringRef,
    conforming_to_uti: CFStringRef,
) -> CFStringRef {
    if tag_class.is_null() || tag.is_null() {
        return nil;
    }
    let tag_class = to_rust_string(env, tag_class).into_owned();
    let tag = to_rust_string(env, tag).into_owned();
    let uti = type_for_tag(&tag_class, &tag);
    let Some(uti) = uti else {
        return nil;
    };
    if !conforming_to_uti.is_null() {
        let parent = to_rust_string(env, conforming_to_uti).into_owned();
        if !type_conforms_to(uti, &parent) {
            return nil;
        }
    }
    ns_string::from_rust_string(env, uti.to_string())
}

fn UTTypeCopyPreferredTagWithClass(
    env: &mut Environment,
    uti: CFStringRef,
    tag_class: CFStringRef,
) -> CFStringRef {
    if uti.is_null() || tag_class.is_null() {
        return nil;
    }
    let uti = to_rust_string(env, uti).into_owned();
    let tag_class = to_rust_string(env, tag_class).into_owned();
    let Some((extension, mime_type)) = known_type(&uti) else {
        return nil;
    };
    let tag = match tag_class.as_str() {
        "public.filename-extension" => extension,
        "public.mime-type" => mime_type,
        _ => return nil,
    };
    ns_string::from_rust_string(env, tag.to_string())
}

fn UTTypeConformsTo(env: &mut Environment, uti: CFStringRef, parent: CFStringRef) -> bool {
    if uti.is_null() || parent.is_null() {
        return false;
    }
    type_conforms_to(&to_rust_string(env, uti), &to_rust_string(env, parent))
}

fn UTTypeIsDeclared(env: &mut Environment, uti: CFStringRef) -> bool {
    !uti.is_null() && is_known_type(&to_rust_string(env, uti))
}

fn UTTypeEqual(env: &mut Environment, lhs: CFStringRef, rhs: CFStringRef) -> bool {
    !lhs.is_null() && !rhs.is_null() && to_rust_string(env, lhs) == to_rust_string(env, rhs)
}

#[cfg(test)]
mod tests {
    use super::{is_known_type, known_type, type_conforms_to, type_for_tag, CONSTANTS};

    #[test]
    fn filename_extensions_resolve_to_known_types_case_insensitively() {
        assert_eq!(
            type_for_tag("public.filename-extension", ".JPG"),
            Some("public.jpeg")
        );
        assert_eq!(
            type_for_tag("public.filename-extension", "pdf"),
            Some("com.adobe.pdf")
        );
        assert_eq!(type_for_tag("public.filename-extension", "unknown"), None);
    }

    #[test]
    fn mime_types_resolve_only_for_supported_tag_class() {
        assert_eq!(
            type_for_tag("public.mime-type", "image/png"),
            Some("public.png")
        );
        assert_eq!(type_for_tag("public.mime-type", ".image/png"), None);
        assert_eq!(type_for_tag("unknown", "image/png"), None);
    }

    #[test]
    fn abstract_types_have_no_preferred_tags() {
        assert_eq!(known_type("public.movie"), None);
        assert_eq!(known_type("public.image"), None);
    }

    #[test]
    fn conformance_includes_identity_and_parent_types() {
        assert!(type_conforms_to("public.jpeg", "public.jpeg"));
        assert!(type_conforms_to("public.jpeg", "public.image"));
        assert!(type_conforms_to("public.jpeg", "public.content"));
        assert!(type_conforms_to("public.jpeg", "public.data"));
        assert!(type_conforms_to("public.jpeg", "public.item"));
        assert!(type_conforms_to("public.json", "public.text"));
        assert!(type_conforms_to("public.json", "public.item"));
        assert!(type_conforms_to("public.vcard", "public.data"));
        assert!(type_conforms_to("public.vcard", "public.content"));
        assert!(type_conforms_to("public.audio", "public.content"));
        assert!(type_conforms_to(
            "public.video",
            "public.audiovisual-content"
        ));
        assert!(type_conforms_to("public.mpeg-4-audio", "public.audio"));
        assert!(type_conforms_to("public.mpeg-4-audio", "public.mpeg4"));
        assert!(type_conforms_to("public.source-code", "public.plain-text"));
        assert!(type_conforms_to("public.aiff-audio", "public.audio"));
        assert!(type_conforms_to(
            "com.apple.quicktime-image",
            "public.image"
        ));
        assert!(type_conforms_to(
            "com.adobe.pdf",
            "public.composite-content"
        ));
        assert!(type_conforms_to("com.adobe.pdf", "public.item"));
        assert!(type_conforms_to("public.folder", "public.item"));
        assert!(type_conforms_to(
            "com.apple.application",
            "public.executable"
        ));
        assert!(!type_conforms_to("public.json", "public.image"));
        assert!(!type_conforms_to("public.data", "public.image"));
        assert!(!type_conforms_to("public.data", "public.content"));
        assert!(!type_conforms_to("public.content", "public.item"));
        assert!(!type_conforms_to("public.content", "public.data"));
        assert!(!type_conforms_to("public.contact", "public.data"));
        assert!(!type_conforms_to("public.archive", "public.data"));
        assert!(!type_conforms_to("public.executable", "public.item"));
        assert!(type_conforms_to("public.data", "public.item"));
        assert!(!type_conforms_to("unknown.uti", "public.data"));
    }

    #[test]
    fn every_exported_uti_constant_is_declared() {
        for (symbol, constant) in CONSTANTS {
            if symbol.starts_with("_kUTType") && *symbol != "_kUTTypeTagSpecificationKey" {
                let super::HostConstant::NSString(uti) = constant else {
                    panic!("unexpected non-string UTI constant {symbol}");
                };
                assert!(is_known_type(uti), "{symbol} ({uti})");
            }
        }
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UTTypeCreatePreferredIdentifierForTag(_, _, _)),
    export_c_func!(UTTypeCopyPreferredTagWithClass(_, _)),
    export_c_func!(UTTypeConformsTo(_, _)),
    export_c_func!(UTTypeIsDeclared(_)),
    export_c_func!(UTTypeEqual(_, _)),
];

// kUTTagClass* / kUTType* are CFStringRef constants exported by
// MobileCoreServices. Apps reference them in `__nl_symbol_ptr` via the
// UTType conversion API even when they never call those functions at
// runtime (e.g. UIImagePickerController media-type checks). Exporting them
// as plain CFString placeholders silences the non-lazy-symbol warning and
// keeps key-comparison-by-identity working.
pub const CONSTANTS: ConstantExports = &[
    (
        "_kUTTagClassFilenameExtension",
        HostConstant::NSString("public.filename-extension"),
    ),
    (
        "_kUTTagClassMIMEType",
        HostConstant::NSString("public.mime-type"),
    ),
    (
        "_kUTTagClassNSPboardType",
        HostConstant::NSString("com.apple.nspboard-type"),
    ),
    (
        "_kUTTagClassOSType",
        HostConstant::NSString("com.apple.ostype"),
    ),
    // The few UTType identifiers most commonly passed by name.
    ("_kUTTypeImage", HostConstant::NSString("public.image")),
    ("_kUTTypeMovie", HostConstant::NSString("public.movie")),
    ("_kUTTypeVideo", HostConstant::NSString("public.video")),
    ("_kUTTypeAudio", HostConstant::NSString("public.audio")),
    ("_kUTTypeData", HostConstant::NSString("public.data")),
    ("_kUTTypeURL", HostConstant::NSString("public.url")),
    ("_kUTTypeText", HostConstant::NSString("public.text")),
    (
        "_kUTTypePlainText",
        HostConstant::NSString("public.plain-text"),
    ),
    ("_kUTTypeJPEG", HostConstant::NSString("public.jpeg")),
    ("_kUTTypePNG", HostConstant::NSString("public.png")),
    // Additional UTType identifiers covering the common image, audio,
    // video and document formats Apple ships with the OS. Values are
    // taken from <https://developer.apple.com/documentation/uniformtypeidentifiers/uttype/system-declared-uniform-type-identifiers>.
    ("_kUTTypeGIF", HostConstant::NSString("com.compuserve.gif")),
    ("_kUTTypeTIFF", HostConstant::NSString("public.tiff")),
    ("_kUTTypeBMP", HostConstant::NSString("com.microsoft.bmp")),
    ("_kUTTypeICO", HostConstant::NSString("com.microsoft.ico")),
    (
        "_kUTTypeJPEG2000",
        HostConstant::NSString("public.jpeg-2000"),
    ),
    (
        "_kUTTypeAppleICNS",
        HostConstant::NSString("com.apple.icns"),
    ),
    (
        "_kUTTypeQuickTimeImage",
        HostConstant::NSString("com.apple.quicktime-image"),
    ),
    (
        "_kUTTypeQuickTimeMovie",
        HostConstant::NSString("com.apple.quicktime-movie"),
    ),
    ("_kUTTypeMPEG", HostConstant::NSString("public.mpeg")),
    ("_kUTTypeMPEG4", HostConstant::NSString("public.mpeg-4")),
    (
        "_kUTTypeMPEG4Audio",
        HostConstant::NSString("public.mpeg-4-audio"),
    ),
    (
        "_kUTTypeAppleProtectedMPEG4Audio",
        HostConstant::NSString("com.apple.protected-mpeg-4-audio"),
    ),
    (
        "_kUTTypeAppleProtectedMPEG4Video",
        HostConstant::NSString("com.apple.protected-mpeg-4-video"),
    ),
    ("_kUTTypeMP3", HostConstant::NSString("public.mp3")),
    ("_kUTTypeAVIMovie", HostConstant::NSString("public.avi")),
    (
        "_kUTTypeAudiovisualContent",
        HostConstant::NSString("public.audiovisual-content"),
    ),
    (
        "_kUTTypeWaveformAudio",
        HostConstant::NSString("com.microsoft.waveform-audio"),
    ),
    (
        "_kUTTypeAIFFAudio",
        HostConstant::NSString("public.aiff-audio"),
    ),
    ("_kUTTypeAUAudio", HostConstant::NSString("public.au-audio")),
    (
        "_kUTTypeMIDIAudio",
        HostConstant::NSString("public.midi-audio"),
    ),
    // Content/format/abstract types.
    ("_kUTTypeContent", HostConstant::NSString("public.content")),
    ("_kUTTypeItem", HostConstant::NSString("public.item")),
    ("_kUTTypeArchive", HostConstant::NSString("public.archive")),
    ("_kUTTypeFolder", HostConstant::NSString("public.folder")),
    (
        "_kUTTypeDirectory",
        HostConstant::NSString("public.directory"),
    ),
    (
        "_kUTTypeApplication",
        HostConstant::NSString("com.apple.application"),
    ),
    ("_kUTTypeBundle", HostConstant::NSString("com.apple.bundle")),
    (
        "_kUTTypeFramework",
        HostConstant::NSString("com.apple.framework"),
    ),
    (
        "_kUTTypeExecutable",
        HostConstant::NSString("public.executable"),
    ),
    // Document types.
    (
        "_kUTTypeUTF8PlainText",
        HostConstant::NSString("public.utf8-plain-text"),
    ),
    (
        "_kUTTypeUTF16PlainText",
        HostConstant::NSString("public.utf16-plain-text"),
    ),
    ("_kUTTypeRTF", HostConstant::NSString("public.rtf")),
    ("_kUTTypeHTML", HostConstant::NSString("public.html")),
    ("_kUTTypeXML", HostConstant::NSString("public.xml")),
    ("_kUTTypeJSON", HostConstant::NSString("public.json")),
    (
        "_kUTTypePropertyList",
        HostConstant::NSString("com.apple.property-list"),
    ),
    (
        "_kUTTypeXMLPropertyList",
        HostConstant::NSString("com.apple.xml-property-list"),
    ),
    (
        "_kUTTypeBinaryPropertyList",
        HostConstant::NSString("com.apple.binary-property-list"),
    ),
    ("_kUTTypePDF", HostConstant::NSString("com.adobe.pdf")),
    ("_kUTTypeVCard", HostConstant::NSString("public.vcard")),
    // VCS / source / build artefacts occasionally referenced by IDE-style apps.
    (
        "_kUTTypeSourceCode",
        HostConstant::NSString("public.source-code"),
    ),
    ("_kUTTypeCSource", HostConstant::NSString("public.c-source")),
    (
        "_kUTTypeObjectiveCSource",
        HostConstant::NSString("public.objective-c-source"),
    ),
    // `kUTTypeTagSpecificationKey` is the dictionary key used by
    // `UTTypeCreateAllIdentifiersForTag()` / `UTTypeCopyDeclaration()`
    // to look up the type's tag specification (filename extensions,
    // MIME types, etc.). The literal value matches Apple's headers.
    // <https://developer.apple.com/documentation/coreservices/kuttypetagspecificationkey>
    (
        "_kUTTypeTagSpecificationKey",
        HostConstant::NSString("UTTypeTagSpecification"),
    ),
];
