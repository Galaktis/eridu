//! File colors: the usual color of each well-known file type (after GitHub Linguist, nudged so
//! look-alikes such as Python and TypeScript stay apart on a dark background), else a hashed hue.

use bevy::color::Color;

use crate::tree::hash;

/// (extensions or whole file names, color as 0xRRGGBB). Lowercase.
const TYPES: &[(&[&str], u32)] = &[
    // Languages.
    (&["py", "pyi", "pyx", "ipynb"], 0x4b7bec),
    (&["ts", "tsx", "mts", "cts"], 0x3fa9f5),
    (&["js", "jsx", "mjs", "cjs"], 0xf1e05a),
    (&["rs"], 0xdea584),
    (&["go"], 0x00d0c0),
    (&["java", "jsp"], 0xb07219),
    (&["kt", "kts"], 0xa97bff),
    (&["scala", "sc"], 0xc22d40),
    (&["groovy", "gradle"], 0x9aa7b0),
    (&["rb", "erb", "rake", "gemspec"], 0xcc342d),
    (&["php"], 0x777bb4),
    (&["c", "h"], 0xa8b9cc),
    (&["cpp", "cc", "cxx", "hpp", "hh", "hxx"], 0xf34b7d),
    (&["cs", "csx"], 0x9b4f96),
    (&["swift"], 0xf05138),
    (&["m", "mm"], 0x438eff),
    (&["dart"], 0x00b4ab),
    (&["lua"], 0x5a6cf0),
    (&["ex", "exs", "heex"], 0x9c6fcb),
    (&["erl", "hrl"], 0xb83998),
    (&["hs"], 0x8f7fd0),
    (&["clj", "cljs", "cljc", "edn"], 0xdb5855),
    (&["r"], 0x198ce7),
    (&["jl"], 0xa270ba),
    (&["zig"], 0xec915c),
    (&["sql", "psql", "ddl"], 0xe38c00),
    (
        &["sh", "bash", "zsh", "fish", "ps1", "psm1", "bat", "cmd"],
        0x89e051,
    ),
    (&["tf", "tfvars", "hcl"], 0x7b42bc),
    (&["proto"], 0xd9a44e),
    (&["graphql", "gql"], 0xe535ab),
    // Web.
    (&["html", "htm", "xhtml"], 0xe34c26),
    (&["css"], 0x8a6fd1),
    (&["scss", "sass", "less"], 0xc6538c),
    (&["vue"], 0x41b883),
    (&["svelte"], 0xff3e00),
    // Documents and data: muted, so code stands out.
    (&["md", "markdown", "rst", "adoc", "txt"], 0x9aa0a6),
    (&["json", "jsonc", "json5"], 0xb5a882),
    (&["yaml", "yml"], 0xa88fa0),
    (&["toml"], 0x9c8b7a),
    (&["xml", "xsd", "wsdl", "plist"], 0x8fa6a0),
    (&["properties", "ini", "cfg", "conf", "env"], 0xa0a0a0),
    (&["csv", "tsv"], 0x7fb77e),
    (&["lock", "sum"], 0x6b6b6b),
    // Test snapshots (insta, Jest) and expected outputs: often thousands, never the point.
    (&["snap", "expect", "stderr", "stdout"], 0x5f6f7a),
    (
        &[
            "gitignore",
            "gitattributes",
            "gitmodules",
            "dockerignore",
            "editorconfig",
        ],
        0x808080,
    ),
    (&["dockerfile", "containerfile"], 0x2496ed),
    (&["makefile", "mk", "cmake"], 0x6d8086),
    // Media.
    // Muted too: repos often carry thousands of icons, which shouldn't drown the code (or
    // look like the pink of Sass).
    (
        &[
            "png", "jpg", "jpeg", "gif", "svg", "ico", "webp", "bmp", "avif",
        ],
        0xd9a88f,
    ),
    (&["ttf", "otf", "woff", "woff2"], 0xd0a0ff),
    (&["mp3", "wav", "ogg", "mp4", "webm", "mov"], 0xff9e80),
];

/// The type of a file: its lowercased extension, or its whole name when it has none
/// (`Dockerfile`, `Makefile`).
pub fn file_type(name: &str) -> String {
    name.rsplit_once('.')
        .map_or(name, |(_, ext)| ext)
        .to_lowercase()
}

/// The color of a file type.
pub fn type_color(file_type: &str) -> Color {
    TYPES
        .iter()
        .find(|(names, _)| names.contains(&file_type))
        .map_or_else(
            || name_color(file_type),
            |&(_, rgb)| Color::srgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
        )
}

/// A stable color for a name (hash → hue), so an unknown type or an author keeps its color across runs.
pub fn name_color(name: &str) -> Color {
    Color::hsl((hash(name) % 360) as f32, 0.75, 0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_types_are_case_insensitive_and_names_count() {
        assert_eq!(file_type("main.RS"), "rs");
        assert_eq!(file_type("Dockerfile"), "dockerfile");
        assert_eq!(file_type(".gitignore"), "gitignore");
        assert_eq!(type_color("py"), Color::srgb_u8(0x4b, 0x7b, 0xec));
        assert_eq!(type_color("unknownext"), name_color("unknownext"));
    }

    fn rgb(color: u32) -> Color {
        Color::srgb_u8((color >> 16) as u8, (color >> 8) as u8, color as u8)
    }

    #[test]
    fn the_last_extension_is_the_type() {
        assert_eq!(file_type("archive.tar.gz"), "gz");
        assert_eq!(file_type("app.test.tsx"), "tsx");
        assert_eq!(file_type("README"), "readme");
        assert_eq!(file_type("Makefile"), "makefile");
        assert_eq!(file_type(".env"), "env");
        assert_eq!(file_type("Photo.JPEG"), "jpeg");
        // Names that end in a dot have an empty type, never the whole name.
        assert_eq!(file_type("odd."), "");
    }

    #[test]
    fn well_known_types_get_their_curated_color() {
        assert_eq!(type_color("rs"), rgb(0xdea584));
        assert_eq!(type_color("ts"), rgb(0x3fa9f5));
        assert_eq!(type_color("tsx"), type_color("ts"));
        assert_eq!(type_color("hpp"), type_color("cpp"));
        assert_eq!(type_color("dockerfile"), rgb(0x2496ed));
        assert_eq!(type_color("gitignore"), rgb(0x808080));
        assert_eq!(type_color("svg"), type_color("png"));
        // What `file_type` gives for real names finds its color.
        assert_eq!(type_color(&file_type("Main.Java")), rgb(0xb07219));
        assert_eq!(type_color(&file_type("Makefile")), rgb(0x6d8086));
        // Look-alikes stay apart.
        assert_ne!(type_color("py"), type_color("ts"));
        assert_ne!(type_color("js"), type_color("ts"));
    }

    #[test]
    fn curated_types_are_listed_once_and_lowercase() {
        let mut seen = std::collections::HashSet::new();
        for (names, _) in TYPES {
            for name in *names {
                assert_eq!(*name, name.to_lowercase(), "{name} is not lowercase");
                assert!(seen.insert(*name), "{name} is listed twice");
            }
        }
    }

    #[test]
    fn type_colors_are_case_sensitive() {
        // `file_type` lowercases first; an uppercase type is unknown, so hashed.
        assert_eq!(type_color("RS"), name_color("RS"));
        assert_ne!(type_color("RS"), type_color("rs"));
    }

    #[test]
    fn hashed_colors_are_stable_and_spread() {
        assert_eq!(name_color("ada@acme.com"), name_color("ada@acme.com"));
        assert_ne!(name_color("ada@acme.com"), name_color("grace@acme.com"));
        // The hue comes from a stable hash, so colors don't change between runs or builds.
        assert_eq!(
            name_color("ada@acme.com"),
            Color::hsl((hash("ada@acme.com") % 360) as f32, 0.75, 0.6)
        );
        assert_eq!(
            name_color(""),
            Color::hsl((0x811c_9dc5u32 % 360) as f32, 0.75, 0.6)
        );
        let Color::Hsla(hsla) = name_color("weird") else {
            panic!("an HSL color");
        };
        assert!((0.0..360.0).contains(&hsla.hue));
        assert_eq!((hsla.saturation, hsla.lightness), (0.75, 0.6));
    }
}
