use crate::config::Icons;
use crate::tree::{Kind, Node};

/// The expansion marker and the file-type glyph for a row, both padded so rows line up.
pub fn prefix(node: &Node, open: bool, icons: Icons) -> (&'static str, &'static str) {
    (marker(node, open, icons), glyph(node, open, icons))
}

fn marker(node: &Node, open: bool, icons: Icons) -> &'static str {
    if node.kind == Kind::File {
        return "  ";
    }

    match (icons, open) {
        (Icons::None, true) => "- ",
        (Icons::None, false) => "+ ",
        (_, true) => "▾ ",
        (_, false) => "▸ ",
    }
}

fn glyph(node: &Node, open: bool, icons: Icons) -> &'static str {
    if icons != Icons::Nerdfont {
        return "";
    }

    match node.kind {
        Kind::Dir if open => "󰝰 ",
        Kind::Dir => "󰉋 ",
        Kind::File => by_name(&node.name),
    }
}

fn by_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();

    let whole_file = match lower.as_str() {
        "cargo.toml" | "cargo.lock" => " ",
        "dockerfile" | "compose.yaml" | "docker-compose.yml" => "󰡨 ",
        "makefile" | "justfile" => " ",
        "license" | "license.md" | "licence" => " ",
        ".gitignore" | ".gitattributes" | ".gitmodules" => " ",
        _ => "",
    };

    if !whole_file.is_empty() {
        return whole_file;
    }

    let extension = lower.rsplit_once('.').map_or("", |(_, ext)| ext);

    match extension {
        "rs" => " ",
        "go" => " ",
        "py" | "pyi" => " ",
        "rb" => " ",
        "js" | "cjs" | "mjs" => " ",
        "ts" | "mts" => " ",
        "jsx" | "tsx" => " ",
        "c" | "h" => " ",
        "cpp" | "cc" | "cxx" | "hpp" => " ",
        "java" => " ",
        "kt" | "kts" => " ",
        "cs" => " ",
        "php" => " ",
        "swift" => " ",
        "lua" => " ",
        "sh" | "bash" | "zsh" | "fish" => " ",
        "nix" => " ",
        "hs" => " ",
        "ex" | "exs" => " ",
        "zig" => " ",
        "html" | "htm" => " ",
        "css" => " ",
        "scss" | "sass" => " ",
        "json" | "jsonc" => " ",
        "toml" => " ",
        "yaml" | "yml" => " ",
        "xml" => "󰗀 ",
        "md" | "markdown" => " ",
        "txt" | "log" => " ",
        "pdf" => " ",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => " ",
        "svg" => "󰜡 ",
        "mp3" | "flac" | "wav" | "ogg" => " ",
        "mp4" | "mkv" | "webm" | "mov" => " ",
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" => " ",
        "sql" | "db" | "sqlite" => " ",
        "ttf" | "otf" | "woff" | "woff2" => " ",
        "lock" => " ",
        _ => " ",
    }
}
