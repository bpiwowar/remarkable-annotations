//! rmapi-js's zod schemas for the JSON files of the cloud (`.content`,
//! `.metadata`, root hash). Parsing validates like zod 4 and returns the
//! object as zod does: the schema's keys first, in schema order, then (for
//! loose objects) the other keys in input order. rmapi-js writes some of these
//! objects back, so the order decides the bytes uploaded.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::js::is_safe_integer;

/// How an object treats keys outside its shape.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    /// Keep them (zod `.loose()`).
    Loose,
    /// Reject them (zod `.strict()`).
    Strict,
}

#[derive(Clone)]
pub(crate) enum T {
    Str,
    Num,
    /// A safe integer.
    Int,
    /// A non-negative safe integer.
    NonNegInt,
    Bool,
    Enum(&'static [&'static str]),
    Datetime,
    Unknown,
    Array(Box<T>),
    /// A record with string keys.
    Record(Box<T>),
    Union(Vec<T>),
    Object(Vec<Field>, Mode),
}

#[derive(Clone)]
pub(crate) struct Field {
    key: &'static str,
    ty: T,
    optional: bool,
    nullable: bool,
}

fn req(key: &'static str, ty: T) -> Field {
    Field {
        key,
        ty,
        optional: false,
        nullable: false,
    }
}

fn opt(key: &'static str, ty: T) -> Field {
    Field {
        key,
        ty,
        optional: true,
        nullable: false,
    }
}

/// `.nullish()`: optional and nullable.
fn nullish(key: &'static str, ty: T) -> Field {
    Field {
        key,
        ty,
        optional: true,
        nullable: true,
    }
}

fn arr(t: T) -> T {
    T::Array(Box::new(t))
}

fn loose(fields: Vec<Field>) -> T {
    T::Object(fields, Mode::Loose)
}

fn datetime_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}(?::[0-9]{2}(?:\.[0-9]+)?)?(?:Z|[+-][0-9]{2}:[0-9]{2})$",
        )
        .unwrap()
    })
}

/// Parses `v` with `t`, returning zod's output.
pub(crate) fn parse(v: &Value, t: &T, path: &str) -> Result<Value, String> {
    let err = |what: &str| Err(format!("{path}: expected {what}"));
    match t {
        T::Str => v.is_string().then(|| v.clone()).map_or(err("string"), Ok),
        T::Num => v.is_number().then(|| v.clone()).map_or(err("number"), Ok),
        T::Int | T::NonNegInt => match v.as_f64() {
            Some(x) if is_safe_integer(x) && (matches!(t, T::Int) || x >= 0.0) => Ok(v.clone()),
            _ => err("integer"),
        },
        T::Bool => v.is_boolean().then(|| v.clone()).map_or(err("boolean"), Ok),
        T::Enum(values) => match v.as_str() {
            Some(s) if values.contains(&s) => Ok(v.clone()),
            _ => err(&format!("one of {values:?}")),
        },
        T::Datetime => match v.as_str() {
            Some(s) if datetime_re().is_match(s) => Ok(v.clone()),
            _ => err("ISO datetime"),
        },
        T::Unknown => Ok(v.clone()),
        T::Array(item) => {
            let Some(a) = v.as_array() else {
                return err("array");
            };
            a.iter()
                .enumerate()
                .map(|(i, x)| parse(x, item, &format!("{path}[{i}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        T::Record(item) => {
            let Some(m) = v.as_object() else {
                return err("object");
            };
            let mut out = Map::new();
            for (k, x) in m {
                out.insert(k.clone(), parse(x, item, &format!("{path}.{k}"))?);
            }
            Ok(Value::Object(out))
        }
        T::Union(variants) => {
            let mut last = None;
            for variant in variants {
                match parse(v, variant, path) {
                    Ok(out) => return Ok(out),
                    Err(e) => last = Some(e),
                }
            }
            Err(last.unwrap_or_else(|| format!("{path}: no variant")))
        }
        T::Object(fields, mode) => {
            let Some(m) = v.as_object() else {
                return err("object");
            };
            let mut out = Map::new();
            for f in fields {
                match m.get(f.key) {
                    None if f.optional => {}
                    None => return Err(format!("{path}.{}: required", f.key)),
                    Some(Value::Null) if f.nullable => {
                        out.insert(f.key.to_owned(), Value::Null);
                    }
                    Some(x) => {
                        out.insert(
                            f.key.to_owned(),
                            parse(x, &f.ty, &format!("{path}.{}", f.key))?,
                        );
                    }
                }
            }
            for (k, x) in m {
                if fields.iter().any(|f| f.key == k) {
                    continue;
                }
                match mode {
                    Mode::Loose => {
                        out.insert(k.clone(), x.clone());
                    }
                    Mode::Strict => return Err(format!("{path}: unrecognized key {k:?}")),
                }
            }
            Ok(Value::Object(out))
        }
    }
}

fn timestamped(ty: T) -> T {
    loose(vec![req("timestamp", T::Str), req("value", ty)])
}

fn tag() -> T {
    loose(vec![req("name", T::Str), req("timestamp", T::Num)])
}

fn document_content(tags: T) -> T {
    let mut fields = vec![
        req("coverPageNumber", T::Int),
        req(
            "documentMetadata",
            loose(vec![
                opt("authors", arr(T::Str)),
                opt("title", T::Str),
                opt("publicationDate", T::Str),
                opt("publisher", T::Str),
            ]),
        ),
        req("extraMetadata", T::Record(Box::new(T::Str))),
        req("fileType", T::Enum(&["epub", "notebook", "pdf"])),
        req("fontName", T::Str),
        req("lineHeight", T::Int),
        req("orientation", T::Enum(&["portrait", "landscape"])),
        req("pageCount", T::NonNegInt),
        req("textAlignment", T::Enum(&["", "justify", "left"])),
        req("textScale", T::Num),
    ];
    let c_page = loose(vec![
        req("id", T::Str),
        req("idx", timestamped(T::Str)),
        opt("template", timestamped(T::Str)),
        opt("redir", timestamped(T::Int)),
        opt("scrollTime", timestamped(T::Datetime)),
        opt("verticalScroll", timestamped(T::Num)),
        opt("deleted", timestamped(T::Int)),
        opt("modifed", T::Str),
    ]);
    let c_pages = loose(vec![
        req("lastOpened", timestamped(T::Str)),
        req("original", timestamped(T::Int)),
        req("pages", arr(c_page)),
        Field {
            key: "uuids",
            ty: arr(loose(vec![
                req("first", T::Str),
                req("second", T::NonNegInt),
            ])),
            optional: false,
            nullable: true,
        },
    ]);
    let orientation = T::Enum(&["portrait", "landscape"]);
    fields.extend([
        opt("cPages", c_pages),
        opt("customZoomCenterX", T::Num),
        opt("customZoomCenterY", T::Num),
        opt("customZoomOrientation", orientation),
        opt("customZoomPageHeight", T::Num),
        opt("customZoomPageWidth", T::Num),
        opt("customZoomScale", T::Num),
        opt("dummyDocument", T::Bool),
        opt("formatVersion", T::NonNegInt),
        opt(
            "keyboardMetadata",
            loose(vec![req("count", T::NonNegInt), req("timestamp", T::Num)]),
        ),
        opt("lastOpenedPage", T::Int),
        opt("margins", T::NonNegInt),
        opt("originalPageCount", T::Int),
        Field {
            key: "pages",
            ty: arr(T::Str),
            optional: true,
            nullable: true,
        },
        nullish(
            "pageTags",
            arr(loose(vec![
                req("name", T::Str),
                req("pageId", T::Str),
                req("timestamp", T::Num),
            ])),
        ),
        nullish("redirectionPageMap", arr(T::Int)),
        opt("sizeInBytes", T::Str),
        opt(
            "transform",
            loose(
                [
                    "m11", "m12", "m13", "m21", "m22", "m23", "m31", "m32", "m33",
                ]
                .into_iter()
                .map(|k| opt(k, T::Num))
                .collect(),
            ),
        ),
        opt("viewBackgroundFilter", T::Enum(&["off", "fullpage"])),
        opt(
            "zoomMode",
            T::Enum(&["bestFit", "customFit", "fitToHeight", "fitToWidth"]),
        ),
        nullish("tags", arr(tags)),
    ]);
    loose(fields)
}

fn template_content() -> T {
    loose(vec![
        opt("id", T::Str),
        req("name", T::Str),
        req("author", T::Str),
        req("iconData", T::Str),
        req("categories", arr(T::Str)),
        opt("labels", arr(T::Str)),
        req("orientation", T::Enum(&["portrait", "landscape"])),
        req("templateVersion", T::Str),
        opt("supportedScreens", arr(T::Enum(&["rm2", "rmPP"]))),
        opt(
            "constants",
            arr(T::Record(Box::new(T::Union(vec![T::Num, T::Str])))),
        ),
        req("items", arr(T::Unknown)),
        opt("formatVersion", T::NonNegInt),
    ])
}

/// The `.content` of an item: collection, template or document (modern or
/// legacy tags), the first matching variant winning.
pub(crate) fn content() -> &'static T {
    static T_: OnceLock<T> = OnceLock::new();
    T_.get_or_init(|| {
        T::Union(vec![
            T::Object(vec![nullish("tags", arr(tag()))], Mode::Strict),
            T::Object(vec![nullish("tags", arr(T::Str))], Mode::Strict),
            template_content(),
            document_content(tag()),
            document_content(T::Str),
        ])
    })
}

/// The `.metadata` of an item.
pub(crate) fn metadata() -> &'static T {
    static T_: OnceLock<T> = OnceLock::new();
    T_.get_or_init(|| {
        loose(vec![
            opt("lastModified", T::Str),
            req("parent", T::Str),
            req("pinned", T::Bool),
            req(
                "type",
                T::Enum(&["DocumentType", "CollectionType", "TemplateType"]),
            ),
            req("visibleName", T::Str),
            opt("lastOpened", T::Str),
            opt("lastOpenedPage", T::Int),
            opt("createdTime", T::Str),
            nullish("deleted", T::Bool),
            nullish("metadatamodified", T::Bool),
            nullish("modified", T::Bool),
            nullish("synced", T::Bool),
            opt("version", T::NonNegInt),
            opt("new", T::Bool),
            opt("source", T::Str),
        ])
    })
}

/// The root hash returned by `GET /sync/v4/root`.
pub(crate) fn root_hash() -> &'static T {
    static T_: OnceLock<T> = OnceLock::new();
    T_.get_or_init(|| {
        loose(vec![
            req("hash", T::Str),
            req("generation", T::Num),
            req("schemaVersion", T::NonNegInt),
        ])
    })
}

/// The root hash returned by `PUT /sync/v3/root`.
pub(crate) fn updated_root_hash() -> &'static T {
    static T_: OnceLock<T> = OnceLock::new();
    T_.get_or_init(|| loose(vec![req("hash", T::Str), req("generation", T::Num)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::js::stringify;

    #[test]
    fn zod_key_order() {
        let input: Value = serde_json::from_str(
            r#"{"zz":1,"tags":[],"pageCount":1,"fileType":"pdf","coverPageNumber":0,
               "documentMetadata":{"x":1,"title":"t"},"extraMetadata":{},"fontName":"",
               "lineHeight":-1,"orientation":"portrait","textAlignment":"justify","textScale":1}"#,
        )
        .unwrap();
        let out = parse(&input, content(), "content").unwrap();
        assert_eq!(
            stringify(&out),
            r#"{"coverPageNumber":0,"documentMetadata":{"title":"t","x":1},"extraMetadata":{},"fileType":"pdf","fontName":"","lineHeight":-1,"orientation":"portrait","pageCount":1,"textAlignment":"justify","textScale":1,"tags":[],"zz":1}"#
        );
        let folder: Value = serde_json::from_str(r#"{"tags":[]}"#).unwrap();
        assert_eq!(
            stringify(&parse(&folder, content(), "c").unwrap()),
            r#"{"tags":[]}"#
        );
    }
}
