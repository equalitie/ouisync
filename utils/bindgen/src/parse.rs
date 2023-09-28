use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};
use syn::{Attribute, BinOp, Expr, ExprBinary, Fields, Item, ItemEnum, Lit, Meta, Visibility};
use thiserror::Error;

#[derive(Default, Debug)]
pub(crate) struct Source {
    pub enums: BTreeMap<String, Enum>,
}

impl Source {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub(crate) struct Enum {
    // TODO: remove this `allow`
    #[allow(unused)]
    pub repr: EnumRepr,
    pub variants: Vec<EnumVariant>,
}

#[derive(Debug)]
pub(crate) struct EnumVariant {
    pub name: String,
    pub value: u64,
}

#[derive(Debug)]
pub(crate) enum EnumRepr {
    U8,
    U16,
    U32,
    U64,
}

impl FromStr for EnumRepr {
    type Err = UnsupportedEnumRepr;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        match input {
            "u8" => Ok(Self::U8),
            "u16" => Ok(Self::U16),
            "u32" => Ok(Self::U32),
            "u64" => Ok(Self::U64),
            _ => Err(UnsupportedEnumRepr),
        }
    }
}

#[derive(Debug)]
pub(crate) struct UnsupportedEnumRepr;

#[derive(Error, Debug)]
pub(crate) enum ParseError {
    #[error("mod '{name}' not found in '{file}'")]
    ModNotFound { file: PathBuf, name: String },
    #[error("syn error")]
    Syn(#[from] syn::Error),
    #[error("io error")]
    Io(#[from] io::Error),
}

pub(crate) fn parse_file(path: impl AsRef<Path>, source: &mut Source) -> Result<(), ParseError> {
    let path = path.as_ref();
    let content = fs::read_to_string(path)?;
    let file = syn::parse_file(&content)?;

    parse_mod(path, file.items, source)
}

fn parse_file_if_exists(path: &Path, source: &mut Source) -> Result<bool, ParseError> {
    match parse_file(path, source) {
        Ok(()) => Ok(true),
        Err(ParseError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn parse_mod(path: &Path, items: Vec<Item>, source: &mut Source) -> Result<(), ParseError> {
    for item in items {
        match item {
            Item::Enum(item) => {
                let name = item.ident.to_string();

                if let Some(value) = parse_enum(item) {
                    source.enums.insert(name, value);
                }
            }
            Item::Mod(item) => match item.content {
                Some((_, items)) => parse_mod(path, items, source)?,
                None => parse_mod_in_file(path, &item.ident.to_string(), source)?,
            },
            _ => (),
        }
    }

    Ok(())
}

fn parse_mod_in_file(
    parent_path: &Path,
    name: &str,
    source: &mut Source,
) -> Result<(), ParseError> {
    // Try ident.rs
    let path = parent_path
        .parent()
        .unwrap()
        .join(name)
        .with_extension("rs");

    if parse_file_if_exists(&path, source)? {
        return Ok(());
    }

    // Try ident/mod.rs
    let path = parent_path.parent().unwrap().join(name).join("mod.rs");

    if parse_file_if_exists(&path, source)? {
        return Ok(());
    }

    // Try self/ident.rs
    let path = parent_path
        .with_extension("")
        .join(name)
        .with_extension("rs");

    if parse_file_if_exists(&path, source)? {
        return Ok(());
    }

    Err(ParseError::ModNotFound {
        file: path.to_owned(),
        name: name.to_owned(),
    })
}

fn parse_enum(item: ItemEnum) -> Option<Enum> {
    if !matches!(item.vis, Visibility::Public(_)) {
        return None;
    }

    let Some(repr) = extract_repr(&item.attrs) else {
        return None;
    };

    let Ok(repr) = repr.parse::<EnumRepr>() else {
        return None;
    };

    let mut next_value = 0;
    let mut variants = Vec::new();

    for variant in item.variants {
        if !matches!(variant.fields, Fields::Unit) {
            println!(
                "enum variant with fields not supported: {}::{}",
                item.ident, variant.ident
            );
            return None;
        }

        let value = if let Some((_, expr)) = variant.discriminant {
            parse_const_int_expr(expr)?
        } else {
            next_value
        };

        next_value = value + 1;

        variants.push(EnumVariant {
            name: variant.ident.to_string(),
            value,
        });
    }

    Some(Enum { repr, variants })
}

fn parse_const_int_expr(expr: Expr) -> Option<u64> {
    match expr {
        Expr::Lit(expr) => parse_int_lit(expr.lit),
        Expr::Binary(expr) => parse_const_int_binary_expr(expr),
        _ => None,
    }
}

fn parse_int_lit(lit: Lit) -> Option<u64> {
    match lit {
        Lit::Int(lit) => {
            if let Ok(value) = lit.base10_parse() {
                Some(value)
            } else {
                println!("int literal overflow: {}", lit.base10_digits());
                None
            }
        }
        Lit::Byte(lit) => Some(lit.value() as _),
        _ => {
            println!("not an int or byte literal");
            None
        }
    }
}

fn parse_const_int_binary_expr(expr: ExprBinary) -> Option<u64> {
    let lhs = parse_const_int_expr(*expr.left)?;
    let rhs = parse_const_int_expr(*expr.right)?;

    match expr.op {
        BinOp::Add(_) => Some(lhs + rhs),
        BinOp::Sub(_) => Some(lhs - rhs),
        BinOp::Mul(_) => Some(lhs * rhs),
        BinOp::Div(_) => Some(lhs / rhs),
        BinOp::Rem(_) => Some(lhs % rhs),
        BinOp::BitXor(_) => Some(lhs ^ rhs),
        BinOp::BitAnd(_) => Some(lhs & rhs),
        BinOp::BitOr(_) => Some(lhs & rhs),
        BinOp::Shl(_) => Some(lhs << rhs),
        BinOp::Shr(_) => Some(lhs >> rhs),
        _ => None,
    }
}

fn extract_repr(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        let Meta::List(meta) = &attr.meta else {
            continue;
        };

        if !meta.path.is_ident("repr") {
            continue;
        }

        return Some(meta.tokens.to_string());
    }

    None
}
