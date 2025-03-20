use std::{
    collections::{BTreeMap, HashSet},
    env,
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, format_err, Context, Result};
use heck::{AsPascalCase, ToPascalCase};
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, Item, Lit, Meta, Pat, PathArguments,
    ReturnType, Signature,
};

fn main() {
    println!("cargo::rerun-if-changed=src/lib.rs");
    println!("cargo::rerun-if-changed=src/state.rs");
    println!("cargo::warning=OUT_DIR={}", env::var("OUT_DIR").unwrap());

    match generate() {
        Ok(()) => (),
        Err(error) => {
            panic!("{:?}", error);
        }
    }
}

/// Info about one variant of the auto-generated `Request` enum.
#[derive(Debug)]
struct RequestVariant {
    /// Documentation comments
    docs: Vec<String>,
    /// Whether the handler for this variant is async
    is_async: bool,
    /// Name of the variant (snake_case)
    name: String,
    /// Variant arguments
    args: Vec<(String, Type)>,
    /// The return type of the handler for this variant
    ret: Type,
    /// In which impl block is the handler defined
    scope: RequestScope,
}

/// Impl block which defines the handler for a given request variant.
#[derive(Clone, Copy, Debug)]
enum RequestScope {
    /// Handler is defined in the `Service` impl block
    Service,
    /// Handler is defined in the `State` impl block
    State,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum Type {
    Unit,
    Scalar(String),
    Option(String),
    Result(Box<Type>, String),
    Vec(String),
    Map(String, String),
    Bytes,
}

impl Type {
    fn serialize_with(&self) -> Option<&str> {
        fn serialize_with(name: &str) -> Option<&str> {
            match name {
                "PeerAddr" | "ShareToken" | "SocketAddr" => Some("helpers::str"),
                _ => None,
            }
        }

        match self {
            Self::Scalar(name) => serialize_with(name),
            Self::Vec(name) => match serialize_with(name) {
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

fn generate() -> Result<()> {
    let mut request_variants = Vec::new();

    let file = parse_file("src/state.rs")?;
    let items = extract_impl_for(&file, "State")?;
    extract_request_variants(RequestScope::State, items, &mut request_variants)?;

    let file = parse_file("src/lib.rs")?;
    let items = extract_impl_for(&file, "Service")?;
    extract_request_variants(RequestScope::Service, items, &mut request_variants)?;

    request_variants.sort_by(|a, b| a.name.cmp(&b.name));

    let response_variants = extract_response_variants(&request_variants);

    fs::write(output_path("request"), generate_request(&request_variants)?)?;
    fs::write(
        output_path("response"),
        generate_response(&response_variants)?,
    )?;
    fs::write(
        output_path("service"),
        generate_service_dispatch(&request_variants)?,
    )?;

    Ok(())
}

fn parse_file(path: impl AsRef<Path>) -> Result<syn::File> {
    let content = fs::read_to_string(path)?;
    Ok(syn::parse_file(&content)?)
}

fn extract_impl_for<'a>(file: &'a syn::File, name: &str) -> Result<&'a [syn::ImplItem]> {
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(item) => Some(item),
            _ => None,
        })
        .find_map(|item| match &*item.self_ty {
            syn::Type::Path(ty) if ty.path.is_ident(name) => Some(item.items.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format_err!("`impl {}` not found", name))
}

fn output_path(base_name: &str) -> PathBuf {
    Path::new(&env::var("OUT_DIR").unwrap())
        .join(base_name)
        .with_extension("rs")
}

fn extract_request_variants(
    scope: RequestScope,
    items: &[ImplItem],
    out: &mut Vec<RequestVariant>,
) -> Result<()> {
    for item in items {
        match item {
            ImplItem::Fn(item) => {
                if is_api_fn(&item.attrs) {
                    out.push(extract_request_variant(scope, &item.attrs, &item.sig)?)
                }
            }
            _ => continue,
        }
    }

    Ok(())
}

fn is_api_fn(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.meta.path().is_ident("api"))
}

fn extract_request_variant(
    scope: RequestScope,
    attrs: &[Attribute],
    sig: &Signature,
) -> Result<RequestVariant> {
    let name = sig.ident.to_string();
    let context = || {
        format!(
            "failed to extract request variant `{}`",
            AsPascalCase(&name)
        )
    };

    let docs = extract_doc_comments(attrs).with_context(context)?;

    let mut args = Vec::new();

    for arg in &sig.inputs {
        let arg = match arg {
            FnArg::Receiver(_) => continue,
            FnArg::Typed(arg) => arg,
        };

        let name = match &*arg.pat {
            Pat::Ident(pat) => pat.ident.to_string(),
            Pat::Wild(_) => continue,
            _ => Err(format_err!("unsupported arg type {:?}", arg.pat)).with_context(context)?,
        };

        if let (RequestScope::Service, "conn_id" | "message_id") = (scope, name.as_str()) {
            continue;
        }

        let ty = extract_type(&arg.ty).with_context(context)?;

        args.push((name, ty));
    }

    let ret = match &sig.output {
        ReturnType::Default => Type::Unit,
        ReturnType::Type(_, ty) => extract_type(ty).with_context(context)?,
    };

    Ok(RequestVariant {
        docs,
        is_async: sig.asyncness.is_some(),
        name,
        args,
        ret,
        scope,
    })
}

fn extract_doc_comments(attrs: &[Attribute]) -> Result<Vec<String>> {
    attrs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(meta) if meta.path.is_ident("doc") => match &meta.value {
                Expr::Lit(expr) => match &expr.lit {
                    Lit::Str(lit) => Some(Ok(lit.value())),
                    _ => Some(Err(format_err!("invalid doc comment: {:?}", expr.lit))),
                },
                _ => Some(Err(format_err!("invalid doc comment: {:?}", meta.value))),
            },
            _ => None,
        })
        .collect()
}

fn extract_type(ty: &syn::Type) -> Result<Type> {
    let path = match ty {
        syn::Type::Path(path) => &path.path,
        syn::Type::Tuple(tuple) => {
            if tuple.elems.is_empty() {
                return Ok(Type::Unit);
            } else {
                bail!("unsupported tuple type: {:?}", tuple);
            }
        }
        _ => bail!("unsupported type: {:?}", ty),
    };

    if path.segments.len() != 1 {
        bail!("qualified types not supported: {:?}", path);
    }

    let first = path.segments.get(0).unwrap();

    match &first.arguments {
        PathArguments::None => Ok(Type::Scalar(first.ident.to_string())),
        PathArguments::AngleBracketed(args) => {
            if first.ident == "Option" {
                if args.args.len() != 1 {
                    bail!(
                        "unexpected number of type arguments for Option (expected: 1, actual: {})",
                        args.args.len()
                    );
                }

                let arg = args.args.get(0).unwrap();
                let arg = extract_generic_type(arg)?;
                let arg = arg
                    .as_scalar()
                    .with_context(|| format!("unsupported type argument for Option: {:?}", arg))?
                    .to_owned();

                return Ok(Type::Option(arg.to_owned()));
            }

            if first.ident == "Result" {
                let (ok, err) = match args.args.len() {
                    1 => {
                        let ok = args.args.get(0).unwrap();
                        let ok = extract_generic_type(ok)?;

                        (ok, "Error".to_string())
                    }
                    2 => {
                        let ok = args.args.get(0).unwrap();
                        let ok = extract_generic_type(ok)?;

                        let err = args.args.get(1).unwrap();
                        let err = extract_generic_type(err)?;
                        let err = err
                            .as_scalar()
                            .with_context(|| format!("unsupported error type: {:?}", err))?
                            .to_owned();

                        (ok, err)
                    }
                    _ => {
                        bail!("unexpected number of type arguments for Result (expected 1 or 2, actual: {})", args.args.len())
                    }
                };

                return Ok(Type::Result(Box::new(ok), err));
            }

            if first.ident == "Vec" {
                if args.args.len() != 1 {
                    bail!(
                        "unexpected number of type arguments for Vec (expected 1, actual: {})",
                        args.args.len()
                    );
                }

                let arg = args.args.get(0).unwrap();
                let arg = extract_generic_type(arg)?;
                let arg = arg
                    .as_scalar()
                    .with_context(|| format!("unsupported type argument for Vec: {:?}", arg))?
                    .to_owned();

                if arg == "u8" {
                    return Ok(Type::Bytes);
                } else {
                    return Ok(Type::Vec(arg));
                }
            }

            if first.ident == "BTreeMap" {
                if args.args.len() != 2 {
                    bail!(
                        "unexpected number of type arguments for BTreeMap (expected 2, actual: {})",
                        args.args.len()
                    );
                }

                let key = args.args.get(0).unwrap();
                let key = extract_generic_type(key)?;
                let key = key
                    .as_scalar()
                    .with_context(|| format!("unsupported map key type: {:?}", key))?
                    .to_owned();

                let val = args.args.get(1).unwrap();
                let val = extract_generic_type(val)?;
                let val = val
                    .as_scalar()
                    .with_context(|| format!("unsupported map value type: {:?}", val))?
                    .to_owned();

                return Ok(Type::Map(key, val));
            }

            bail!("unsupported type: {:?}", first.ident)
        }
        PathArguments::Parenthesized(_) => bail!("unsupported type: {:?}", first.ident),
    }
}

fn extract_generic_type(arg: &GenericArgument) -> Result<Type> {
    match arg {
        GenericArgument::Type(ty) => extract_type(ty),
        _ => bail!("unsupported generic argument: {:?}", arg),
    }
}

fn extract_response_variants(requests: &[RequestVariant]) -> BTreeMap<String, Type> {
    let types: HashSet<_> = requests
        .iter()
        .map(|request| request.ret.unwrap())
        .collect();

    types
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
        .collect()
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

fn generate_request(requests: &[RequestVariant]) -> Result<String> {
    let mut out = String::new();

    writeln!(
        out,
        "#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]"
    )?;
    writeln!(out, "#[serde(rename_all = \"snake_case\")]")?;
    writeln!(out, "pub enum Request {{")?;

    for request in requests {
        for doc in &request.docs {
            writeln!(out, "{INDENT}///{doc}")?;
        }

        write!(out, "{INDENT}{}", AsPascalCase(&request.name))?;

        if !request.args.is_empty() {
            writeln!(out, " {{")?;

            for (name, ty) in &request.args {
                if let Some(s) = ty.serialize_with() {
                    writeln!(out, "{INDENT}{INDENT}#[serde(with = \"{s}\")]")?;
                }

                writeln!(out, "{INDENT}{INDENT}{name}: {ty},")?;
            }

            write!(out, "{INDENT}}}")?;
        }

        writeln!(out, ",")?;
    }

    writeln!(out, "}}")?;

    Ok(out)
}

fn generate_response(responses: &BTreeMap<String, Type>) -> Result<String> {
    let mut out = String::new();

    writeln!(
        out,
        "#[derive(Eq, PartialEq, Debug, Serialize, Deserialize)]"
    )?;
    writeln!(out, "#[serde(rename_all = \"snake_case\")]")?;
    writeln!(out, "#[allow(clippy::large_enum_variant)]")?;
    writeln!(out, "pub enum Response {{")?;

    // generate the `Response` enum
    for (name, ty) in responses {
        write!(out, "{INDENT}{name}")?;

        match ty {
            Type::Unit => (),
            _ => {
                write!(out, "(")?;

                if let Some(s) = ty.serialize_with() {
                    write!(out, "#[serde(with = \"{s}\")] ")?;
                }

                write!(out, "{ty})")?;
            }
        }
        writeln!(out, ",")?;
    }

    writeln!(out, "}}")?;

    writeln!(out)?;

    // generate `impl From<T> for Response`, `impl TryFrom<Response> for T` and `impl TryFrom<Response> for Option<T>`
    for (name, ty) in responses {
        if matches!(ty, Type::Unit) {
            continue;
        }

        writeln!(out, "impl From<{ty}> for Response {{")?;
        writeln!(out, "{INDENT}fn from(value: {ty}) -> Self {{")?;
        writeln!(out, "{INDENT}{INDENT}Self::{name}(value)")?;
        writeln!(out, "{INDENT}}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;

        writeln!(out, "impl TryFrom<Response> for {ty} {{")?;
        writeln!(out, "{INDENT}type Error = UnexpectedResponse;")?;
        writeln!(
            out,
            "{INDENT}fn try_from(response: Response) -> Result<Self, Self::Error> {{"
        )?;
        writeln!(out, "{INDENT}{INDENT}match response {{")?;
        writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}Response::{name}(value) => Ok(value),"
        )?;
        writeln!(out, "{INDENT}{INDENT}{INDENT}_ => Err(UnexpectedResponse),")?;
        writeln!(out, "{INDENT}{INDENT}}}")?;
        writeln!(out, "{INDENT}}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;

        writeln!(out, "impl TryFrom<Response> for Option<{ty}> {{")?;
        writeln!(out, "{INDENT}type Error = UnexpectedResponse;")?;
        writeln!(
            out,
            "{INDENT}fn try_from(value: Response) -> Result<Self, Self::Error> {{"
        )?;
        writeln!(out, "{INDENT}{INDENT}match value {{")?;
        writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}Response::{name}(value) => Ok(Some(value)),"
        )?;
        writeln!(out, "{INDENT}{INDENT}{INDENT}Response::None => Ok(None),")?;
        writeln!(out, "{INDENT}{INDENT}{INDENT}_ => Err(UnexpectedResponse),")?;
        writeln!(out, "{INDENT}{INDENT}}}")?;
        writeln!(out, "}}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }

    Ok(out)
}

fn generate_service_dispatch(requests: &[RequestVariant]) -> Result<String> {
    let mut out = String::new();

    writeln!(out, "#[allow(clippy::let_unit_value)]")?;
    writeln!(out, "impl Service {{")?;
    writeln!(out, "{INDENT}async fn dispatch(")?;
    writeln!(out, "{INDENT}{INDENT}&mut self,")?;
    writeln!(out, "{INDENT}{INDENT}conn_id: ConnectionId,")?;
    writeln!(out, "{INDENT}{INDENT}message: Message<Request>,")?;
    writeln!(out, "{INDENT}) -> Result<Response, ProtocolError> {{")?;

    writeln!(out, "{INDENT}{INDENT}match message.payload {{")?;

    for request in requests {
        write!(
            out,
            "{INDENT}{INDENT}{INDENT}Request::{}",
            AsPascalCase(&request.name)
        )?;

        if !request.args.is_empty() {
            writeln!(out, " {{")?;

            for (name, _) in &request.args {
                writeln!(out, "{INDENT}{INDENT}{INDENT}{INDENT}{name},")?;
            }

            write!(out, "{INDENT}{INDENT}{INDENT}}}")?;
        }

        writeln!(out, " => {{")?;

        match request.scope {
            RequestScope::State => {
                writeln!(
                    out,
                    "{INDENT}{INDENT}{INDENT}{INDENT}let ret = self.state.{}(",
                    request.name
                )?;
            }
            RequestScope::Service => {
                writeln!(
                    out,
                    "{INDENT}{INDENT}{INDENT}{INDENT}let ret = self.{}(",
                    request.name
                )?;

                // Pass `conn_id` and `message_id` to subscription handlers.
                if request.name.contains("subscribe") {
                    writeln!(out, "{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}conn_id,")?;
                    writeln!(out, "{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}message.id,")?;
                }
            }
        }

        for (name, ty) in &request.args {
            write!(out, "{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}{name}")?;

            if matches!(ty, Type::Bytes) {
                write!(out, ".into()")?;
            }

            writeln!(out, ",")?;
        }

        writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}{INDENT}){}{};",
            if request.is_async { ".await" } else { "" },
            match request.ret {
                Type::Result(..) => "?",
                _ => "",
            },
        )?;

        writeln!(out, "{INDENT}{INDENT}{INDENT}{INDENT}")?;
        writeln!(out, "{INDENT}{INDENT}{INDENT}{INDENT}Ok(ret.into())")?;

        writeln!(out, "{INDENT}{INDENT}{INDENT}}}")?;
    }

    writeln!(out, "{INDENT}{INDENT}}}")?;
    writeln!(out, "{INDENT}}}")?;
    writeln!(out, "}}")?;

    Ok(out)
}

const INDENT: &str = "    ";
