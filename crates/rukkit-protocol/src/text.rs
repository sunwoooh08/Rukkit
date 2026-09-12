//! Chat/text components.
//!
//! Components travel in two shapes and both are needed: the status response
//! carries them as JSON, while play and configuration packets carry them as
//! network NBT (since 1.20.3). One struct produces both.

use serde::{Deserialize, Serialize};

use crate::nbt::{NbtCompound, NbtList, NbtTag};

/// The sixteen named colours vanilla accepts, plus the shorthand for hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    DarkBlue,
    DarkGreen,
    DarkAqua,
    DarkRed,
    DarkPurple,
    Gold,
    Gray,
    DarkGray,
    Blue,
    Green,
    Aqua,
    Red,
    LightPurple,
    Yellow,
    White,
}

impl NamedColor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::DarkBlue => "dark_blue",
            Self::DarkGreen => "dark_green",
            Self::DarkAqua => "dark_aqua",
            Self::DarkRed => "dark_red",
            Self::DarkPurple => "dark_purple",
            Self::Gold => "gold",
            Self::Gray => "gray",
            Self::DarkGray => "dark_gray",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Aqua => "aqua",
            Self::Red => "red",
            Self::LightPurple => "light_purple",
            Self::Yellow => "yellow",
            Self::White => "white",
        }
    }
}

/// A text component: literal or translatable text plus styling and children.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Component {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlined: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obfuscated: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<Component>,
}

impl Component {
    /// A literal-text component.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// A component resolved against the client's language file.
    #[must_use]
    pub fn translate(key: impl Into<String>) -> Self {
        Self {
            translate: Some(key.into()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn color(mut self, color: NamedColor) -> Self {
        self.color = Some(color.as_str().to_owned());
        self
    }

    /// Sets an `#rrggbb` colour.
    #[must_use]
    pub fn hex_color(mut self, rgb: u32) -> Self {
        self.color = Some(format!("#{:06X}", rgb & 0xFF_FFFF));
        self
    }

    #[must_use]
    pub fn bold(mut self, value: bool) -> Self {
        self.bold = Some(value);
        self
    }

    #[must_use]
    pub fn italic(mut self, value: bool) -> Self {
        self.italic = Some(value);
        self
    }

    #[must_use]
    pub fn underlined(mut self, value: bool) -> Self {
        self.underlined = Some(value);
        self
    }

    #[must_use]
    pub fn strikethrough(mut self, value: bool) -> Self {
        self.strikethrough = Some(value);
        self
    }

    #[must_use]
    pub fn obfuscated(mut self, value: bool) -> Self {
        self.obfuscated = Some(value);
        self
    }

    /// Appends a child component, which inherits this one's styling.
    #[must_use]
    pub fn append(mut self, child: Component) -> Self {
        self.extra.push(child);
        self
    }

    /// Serializes to the JSON form used by the status response.
    ///
    /// Falls back to an empty literal if serialization somehow fails, so that a
    /// styling bug can never take down a connection.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"text":""}"#.to_owned())
    }

    /// Converts to the network NBT form used by play and configuration packets.
    #[must_use]
    pub fn to_nbt(&self) -> NbtTag {
        let mut c = NbtCompound::new();
        if let Some(text) = &self.text {
            c.insert("text", text.as_str());
        }
        if let Some(translate) = &self.translate {
            c.insert("translate", translate.as_str());
        }
        if let Some(color) = &self.color {
            c.insert("color", color.as_str());
        }
        for (key, value) in [
            ("bold", self.bold),
            ("italic", self.italic),
            ("underlined", self.underlined),
            ("strikethrough", self.strikethrough),
            ("obfuscated", self.obfuscated),
        ] {
            if let Some(value) = value {
                c.insert(key, value);
            }
        }
        if !self.extra.is_empty() {
            let children: Vec<NbtTag> = self.extra.iter().map(Component::to_nbt).collect();
            // Every child is a compound, so the list is homogeneous by
            // construction and `new` cannot fail.
            if let Some(list) = NbtList::new(children) {
                c.insert("extra", list);
            }
        }
        NbtTag::Compound(c)
    }

    /// Flattens to plain text, discarding styling. Used for console logs.
    #[must_use]
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        self.write_plain(&mut out);
        out
    }

    fn write_plain(&self, out: &mut String) {
        if let Some(text) = &self.text {
            out.push_str(text);
        } else if let Some(key) = &self.translate {
            // Without the client's language file the key is the best we have.
            out.push_str(key);
        }
        for child in &self.extra {
            child.write_plain(out);
        }
    }
}

impl From<&str> for Component {
    fn from(value: &str) -> Self {
        Self::text(value)
    }
}

impl From<String> for Component {
    fn from(value: String) -> Self {
        Self::text(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_component_serializes_without_null_fields() {
        let json = Component::text("hi").to_json();
        assert_eq!(json, r#"{"text":"hi"}"#);
    }

    #[test]
    fn styling_appears_only_when_set() {
        let json = Component::text("hi")
            .color(NamedColor::Red)
            .bold(true)
            .to_json();
        assert!(json.contains(r#""color":"red""#), "{json}");
        assert!(json.contains(r#""bold":true"#), "{json}");
        assert!(!json.contains("italic"), "{json}");
    }

    #[test]
    fn hex_colors_are_six_digit_uppercase() {
        let c = Component::text("x").hex_color(0x00_FF7F);
        assert_eq!(c.color.as_deref(), Some("#00FF7F"));
    }

    #[test]
    fn children_round_trip_through_json() {
        let original = Component::text("a")
            .color(NamedColor::Gold)
            .append(Component::text("b").italic(true));
        let json = original.to_json();
        let parsed: Component = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn plain_text_flattens_children_in_order() {
        let c = Component::text("Hello, ")
            .append(Component::text("world").bold(true))
            .append(Component::text("!"));
        assert_eq!(c.plain_text(), "Hello, world!");
    }

    #[test]
    fn nbt_form_carries_the_same_fields() {
        let c = Component::text("hi")
            .color(NamedColor::Red)
            .bold(true)
            .append(Component::text("!"));
        let NbtTag::Compound(compound) = c.to_nbt() else {
            panic!("expected compound");
        };
        assert_eq!(compound.get("text").and_then(NbtTag::as_str), Some("hi"));
        assert_eq!(compound.get("color").and_then(NbtTag::as_str), Some("red"));
        assert_eq!(compound.get("bold").and_then(NbtTag::as_i64), Some(1));
        assert!(compound.get("italic").is_none());

        match compound.get("extra") {
            Some(NbtTag::List(list)) => assert_eq!(list.len(), 1),
            other => panic!("expected extra list, got {other:?}"),
        }
    }

    #[test]
    fn nbt_form_survives_a_binary_round_trip() {
        use crate::nbt;
        use crate::reader::PacketReader;

        let tag = Component::text("색 \u{1F600}")
            .color(NamedColor::Aqua)
            .to_nbt();
        let mut buf = Vec::new();
        nbt::write_network(&tag, &mut buf);
        let decoded = nbt::read_network(&mut PacketReader::new(&buf)).unwrap();
        assert_eq!(decoded, tag);
    }
}
