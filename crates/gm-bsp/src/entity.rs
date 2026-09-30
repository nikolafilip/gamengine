//! The entity lump: `{ "key" "value" ... }` blocks.

use glam::Vec3;

use crate::BspError;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entity {
    pub props: Vec<(String, String)>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn f32(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }

    /// Parse a `"x y z"` triple.
    pub fn vec3(&self, key: &str) -> Option<Vec3> {
        let mut it = self
            .get(key)?
            .split_whitespace()
            .map(|s| s.parse::<f32>().ok());
        let v = Vec3::new(it.next()??, it.next()??, it.next()??);
        Some(v)
    }

    pub fn origin(&self) -> Option<Vec3> {
        self.vec3("origin")
    }
}

pub fn parse_entities(text: &str) -> Result<Vec<Entity>, BspError> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut current: Option<Entity> = None;
    let mut pending_key: Option<String> = None;

    let read_string =
        |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>| -> Result<String, BspError> {
            let mut s = String::new();
            for (_, c) in chars.by_ref() {
                if c == '"' {
                    return Ok(s);
                }
                s.push(c);
            }
            Err(BspError::Invalid(
                "unterminated string in entity lump".into(),
            ))
        };

    while let Some(&(_, c)) = chars.peek() {
        match c {
            '{' => {
                chars.next();
                if current.is_some() {
                    return Err(BspError::Invalid("nested '{' in entity lump".into()));
                }
                current = Some(Entity::default());
            }
            '}' => {
                chars.next();
                let e = current
                    .take()
                    .ok_or_else(|| BspError::Invalid("'}' without '{' in entity lump".into()))?;
                if pending_key.take().is_some() {
                    return Err(BspError::Invalid("key without value in entity lump".into()));
                }
                out.push(e);
            }
            '"' => {
                chars.next();
                let s = read_string(&mut chars)?;
                match (&mut current, pending_key.take()) {
                    (Some(e), Some(k)) => e.props.push((k, s)),
                    (Some(_), None) => pending_key = Some(s),
                    (None, _) => {
                        return Err(BspError::Invalid(
                            "string outside of an entity block".into(),
                        ));
                    }
                }
            }
            '/' => {
                // `// comment` to end of line.
                chars.next();
                if matches!(chars.peek(), Some(&(_, '/'))) {
                    for (_, c) in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    return Err(BspError::Invalid("unexpected '/' in entity lump".into()));
                }
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            other => {
                return Err(BspError::Invalid(format!(
                    "unexpected character {other:?} in entity lump"
                )));
            }
        }
    }
    if current.is_some() {
        return Err(BspError::Invalid("unterminated entity block".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_values() {
        let text = r#"
{
"classname" "worldspawn"
"wad" "base.wad"
}
// a comment
{
"classname" "light"
"origin" "-128 -112 160"
"light" "300"
"_color" "255 220 170"
}
"#;
        let ents = parse_entities(text).unwrap();
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0].classname(), "worldspawn");
        assert_eq!(ents[0].get("wad"), Some("base.wad"));
        assert_eq!(ents[1].origin(), Some(Vec3::new(-128.0, -112.0, 160.0)));
        assert_eq!(ents[1].f32("light"), Some(300.0));
        assert_eq!(ents[1].vec3("_color"), Some(Vec3::new(255.0, 220.0, 170.0)));
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse_entities("{ \"a\" }").is_err());
        assert!(parse_entities("{ \"a\" \"b\"").is_err());
        assert!(parse_entities("\"a\" \"b\"").is_err());
        assert!(parse_entities("{ { } }").is_err());
    }
}
