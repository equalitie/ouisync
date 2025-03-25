mod parse;

use std::{collections::HashSet, fmt, path::Path};

use anyhow::Result;
use heck::ToPascalCase;
use serde::Serialize;

/// Items extracted from the rust codebase used as the source for codegen.
#[derive(Default, Debug, Serialize)]
pub struct Context {
    pub request: Request,
    pub response: Response,
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

#[derive(Default, Debug, Serialize)]
pub struct Request {
    pub variants: Vec<(String, RequestVariant)>,
}

impl Request {
    fn sort(&mut self) {
        self.variants.sort_by(|(a, _), (b, _)| a.cmp(b));
    }
}

#[derive(Debug, Serialize)]
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

#[derive(Default, Debug, Serialize)]
pub struct Response {
    pub variants: Vec<(String, Type)>,
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
                let name = match &ty {
                    Type::Unit => "None".to_owned(),
                    Type::Scalar(v) => make_response_name(v),
                    Type::Vec(v) => pluralize(&make_response_name(v)),
                    Type::Map(_, v) => pluralize(&make_response_name(v)),
                    Type::Bytes => "Bytes".to_owned(),
                    Type::Option(_) | Type::Result(_, _) => unreachable!(),
                };

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

fn make_response_name(name: &str) -> String {
    let name = name.strip_suffix("Handle").unwrap_or(name);
    let name = if name == "PathBuf" { "Path" } else { name };
    name.to_pascal_case()
}

fn pluralize(word: &str) -> String {
    if let Some(word) = word.strip_suffix('y') {
        format!("{}ies", word)
    } else {
        format!("{}s", word)
    }
}

// /// Enum with data
// #[derive(Serialize, Debug)]
// pub struct Union {
//     docs: Docs,
//     variants: Vec<(String, UnionVariant)>,
// }

// #[derive(Serialize, Debug)]
// pub struct UnionVariant {
//     docs: Docs,
//     fields: Fields,
// }

// #[derive(Serialize, Debug)]
// pub enum Fields {
//     Named(Vec<(String, Arg)>),
//     Unnamed(Vec<Arg>),
// }

// #[derive(Serialize, Debug)]
// pub struct Arg {
//     pub docs: Docs,
//     pub ty: Ty,
// }

#[derive(Default, Debug, Serialize)]
pub struct Docs {
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
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
