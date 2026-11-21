//! Named frame sizes: the app's frame templates, and the devices a page's
//! prototype is played in, which the player looks up by name to lay a screen out
//! at a device's size.

use kurbo::Size;

/// One named size: the label a list shows, and the box it writes.
pub type Preset = (&'static str, f64, f64);
/// A heading and the sizes under it.
pub type PresetGroup = (&'static str, &'static [Preset]);

/// The frame sizes the app's *Frame templates* offer, by group; the phone and
/// tablet groups are the devices a page can play in.
///
/// One world unit is one pixel; print sizes are points at 72/inch (A4 is 595×842).
/// Portrait where a device has one. Two entries may not share a box, since the
/// app names a frame by the first match.
pub const FRAME_PRESETS: &[PresetGroup] = &[
    (
        "PHONE",
        &[
            ("iPhone 16", 393.0, 852.0),
            ("iPhone 16 Pro Max", 440.0, 956.0),
            ("iPhone SE", 375.0, 667.0),
            ("Android compact", 412.0, 917.0),
            ("Pixel 9", 412.0, 923.0),
            ("Pixel 9 Pro XL", 448.0, 998.0),
            ("Galaxy S24", 360.0, 780.0),
            ("Galaxy S24 Ultra", 384.0, 824.0),
        ],
    ),
    (
        "TABLET",
        &[
            ("iPad mini", 744.0, 1133.0),
            ("iPad Pro 11\"", 834.0, 1194.0),
            ("iPad Pro 13\"", 1032.0, 1376.0),
            ("Android tablet", 800.0, 1280.0),
            ("Surface Pro", 912.0, 1368.0),
        ],
    ),
    (
        "DESKTOP",
        &[
            ("Desktop", 1440.0, 1024.0),
            ("MacBook Air", 1280.0, 832.0),
            ("Laptop 13\"", 1280.0, 800.0),
            ("Full HD", 1920.0, 1080.0),
        ],
    ),
    (
        "PRINT",
        &[
            ("A3", 842.0, 1191.0),
            ("A4", 595.0, 842.0),
            ("A5", 420.0, 595.0),
            ("US Letter", 612.0, 792.0),
        ],
    ),
    (
        "SOCIAL",
        &[
            ("Square post", 1080.0, 1080.0),
            ("Story", 1080.0, 1920.0),
            ("Link preview", 1200.0, 630.0),
            ("Banner", 1500.0, 500.0),
        ],
    ),
];

/// The groups a page's prototype can play in as its device.
pub const DEVICE_GROUPS: [&str; 2] = ["PHONE", "TABLET"];

/// The portrait size of the device named `name`, `None` for a name no phone or
/// tablet has.
pub fn device_size(name: &str) -> Option<Size> {
    FRAME_PRESETS
        .iter()
        .filter(|(g, _)| DEVICE_GROUPS.contains(g))
        .flat_map(|(_, entries)| entries.iter())
        .find(|(n, _, _)| *n == name)
        .map(|(_, w, h)| Size::new(*w, *h))
}
