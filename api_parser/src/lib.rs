mod parse;

use std::{collections::HashSet, fmt, path::Path, str::FromStr};

use anyhow::{format_err, Error, Result};
use heck::ToPascalCase;

/// Items extracted from the rust codebase used as the source for codegen.
#[derive(Default, Debug)]
pub struct Context {
    pub request: Request,
    pub response: Response,
    pub items: Vec<(String, Item)>,
}

impl Context {
    pub fn parse(sources: &[impl AsRef<Path>]) -> Result<Self> {
        let mut ctx = Self::default();

        for source in sources {
            parse::parse_file(&mut ctx, source.as_ref(), true)?;
        }

        ctx.request.sort();
        ctx.response = Response::from(&ctx.request);

        Ok(ctx)
    }
}

#[derive(Default, Debug)]
pub struct Request {
    pub variants: Vec<(String, RequestVariant)>,
}

impl Request {
    fn sort(&mut self) {
        self.variants.sort_by(|(a, _), (b, _)| a.cmp(b));
    }

    pub fn to_enum(&self) -> ComplexEnum {
        let variants = self
            .variants
            .iter()
            .map(|(name, variant)| {
                (
                    name.to_pascal_case(),
                    ComplexVariant {
                        docs: Docs::default(),
                        fields: variant
                            .fields
                            .iter()
                            .map(|(name, ty)| {
                                (
                                    name.clone(),
                                    Field {
                                        docs: Docs::default(),
                                        ty: ty.clone(),
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .collect();

        ComplexEnum {
            docs: Docs::default(),
            variants,
        }
    }
}

#[derive(Debug)]
pub struct RequestVariant {
    pub docs: Docs,
    pub fields: Vec<(String, Type)>,
    /// The return type of the handler for this variant
    pub ret: Type,
    /// Whether the handler for this variant is async
    pub is_async: bool,
    /// In which impl block is the handler defined
    pub scope: String,
}

#[derive(Default, Debug)]
pub struct Response {
    pub variants: Vec<(String, Type)>,
}

impl Response {
    pub fn to_enum(&self) -> ComplexEnum {
        let variants = self
            .variants
            .iter()
            .map(|(name, ty)| {
                (
                    name.to_pascal_case(),
                    ComplexVariant {
                        docs: Docs::default(),
                        fields: match ty {
                            Type::Unit => vec![],
                            _ => vec![(
                                "value".to_owned(),
                                Field {
                                    docs: Docs::default(),
                                    ty: ty.clone(),
                                },
                            )],
                        },
                    },
                )
            })
            .collect();

        ComplexEnum {
            docs: Docs::default(),
            variants,
        }
    }
}

impl<'a> From<&'a Request> for Response {
    fn from(request: &'a Request) -> Self {
        let types: HashSet<_> = request
            .variants
            .iter()
            .map(|(_, variant)| variant.ret.unwrap())
            .collect();

        let mut variants: Vec<_> = types
            .into_iter()
            .map(|ty| {
                let name = ty.to_response_variant_name();
                (name, ty)
            })
            .chain([
                ("RepositoryEvent".to_owned(), Type::Unit),
                (
                    "NetworkEvent".to_owned(),
                    Type::Scalar("NetworkEvent".to_owned()),
                ),
                ("StateMonitorEvent".to_owned(), Type::Unit),
            ])
            .collect();
        variants.sort_by(|(a, _), (b, _)| a.cmp(b));

        Response { variants }
    }
}

pub trait ToResponseVariantName {
    fn to_response_variant_name(&self) -> String;
}

impl ToResponseVariantName for Type {
    fn to_response_variant_name(&self) -> String {
        match self {
            Type::Unit => "None".to_owned(),
            Type::Scalar(s) => s.to_response_variant_name(),
            Type::Vec(s) => pluralize(&s.to_response_variant_name()),
            Type::Map(_, v) => pluralize(&v.to_response_variant_name()),
            Type::Bytes => "Bytes".to_owned(),
            Type::Option(s) => s.to_response_variant_name(),
            Type::Result(t, _) => t.to_response_variant_name(),
        }
    }
}

impl ToResponseVariantName for str {
    fn to_response_variant_name(&self) -> String {
        let name = self.strip_suffix("Handle").unwrap_or(self);
        let name = if name == "PathBuf" { "Path" } else { name };
        name.to_pascal_case()
    }
}

fn pluralize(word: &str) -> String {
    if let Some(word) = word.strip_suffix('y') {
        format!("{}ies", word)
    } else {
        format!("{}s", word)
    }
}

#[derive(Debug)]
pub enum Item {
    SimpleEnum(SimpleEnum),
    ComplexEnum(ComplexEnum),
    Struct(Struct),
    Newtype(Newtype),
}

/// Simple enum (C-style enums)
#[derive(Debug)]
pub struct SimpleEnum {
    pub docs: Docs,
    pub repr: EnumRepr,
    pub variants: Vec<(String, SimpleVariant)>,
}

#[derive(Debug)]
pub struct SimpleVariant {
    pub docs: Docs,
    pub value: u64,
}

#[derive(Debug)]
pub enum EnumRepr {
    U8,
    U16,
    U32,
    U64,
}

impl FromStr for EnumRepr {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        match input {
            "u8" => Ok(Self::U8),
            "u16" => Ok(Self::U16),
            "u32" => Ok(Self::U32),
            "u64" => Ok(Self::U64),
            _ => Err(format_err!("unsupported enum repr: {input}")),
        }
    }
}

/// Complex enum (discriminated union / algebraic data type / sum type)
#[derive(Debug)]
pub struct ComplexEnum {
    pub docs: Docs,
    pub variants: Vec<(String, ComplexVariant)>,
}

#[derive(Debug)]
pub struct ComplexVariant {
    pub docs: Docs,
    pub fields: Vec<(String, Field)>,
}

#[derive(Debug)]
pub struct Struct {
    pub docs: Docs,
    pub fields: Vec<(String, Field)>,
}

#[derive(Debug)]
pub struct Newtype {
    pub docs: Docs,
    pub ty: Type,
}

#[derive(Debug)]
pub struct Field {
    pub docs: Docs,
    pub ty: Type,
}

#[derive(Default, Clone, Debug)]
pub struct Docs {
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Type {
    Unit,
    Scalar(String),
    Option(String),
    Result(Box<Type>, String),
    Vec(String),
    Map(String, String),
    Bytes,
}

impl Type {
    pub fn serde_with(&self) -> Option<&str> {
        fn serde_with(name: &str) -> Option<&str> {
            match name {
                "PeerAddr" | "ShareToken" | "SocketAddr" => Some("helpers::str"),
                "Duration" => Some("helpers::millis"),
                _ => None,
            }
        }

        match self {
            Self::Scalar(name) => serde_with(name),
            Self::Vec(name) => match serde_with(name) {
                Some("helpers::str") => Some("helpers::strs"),
                Some(_) => unreachable!(),
                None => None,
            },
            _ => None,
        }
    }

    fn as_scalar(&self) -> Option<&str> {
        match self {
            Self::Scalar(name) => Some(name),
            _ => None,
        }
    }

    fn unwrap(&self) -> Self {
        match self {
            Self::Option(name) => Self::Scalar(name.clone()),
            Self::Result(ty, _) => ty.unwrap(),
            _ => self.clone(),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(f, "()"),
            Self::Scalar(ty) => write!(f, "{ty}"),
            Self::Option(ty) => write!(f, "Option<{ty}>"),
            Self::Result(ty, error) => write!(f, "Result<{ty}, {error}>"),
            Self::Vec(ty) => write!(f, "Vec<{ty}>"),
            Self::Map(k, v) => write!(f, "BTreeMap<{k}, {v}>"),
            Self::Bytes => write!(f, "Bytes"),
        }
    }
}
