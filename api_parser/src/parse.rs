use std::{fs, io, path::Path};

use anyhow::{bail, format_err, Context as _, Result};
use heck::AsPascalCase;
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, Item, Lit, Meta, Pat, PathArguments,
    ReturnType, Signature,
};

use crate::{Context, Docs, RequestVariant, Type};

pub(crate) fn parse_file(ctx: &mut Context, path: &Path, fail_on_not_found: bool) -> Result<bool> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if fail_on_not_found {
                return Err(error.into());
            } else {
                return Ok(false);
            }
        }
        Err(error) => return Err(error.into()),
    };
    let file = syn::parse_file(&content)?;

    parse_mod(ctx, path, file.items)?;

    Ok(true)
}

fn parse_mod(ctx: &mut Context, path: &Path, items: Vec<Item>) -> Result<()> {
    for item in items {
        match item {
            // Item::Enum(item) => {
            //     let name = item.ident.to_string();

            //     if let Some(value) = parse_enum(item) {
            //         source.enums.insert(name, value);
            //     }
            // }
            Item::Mod(item) => match item.content {
                Some((_, items)) => parse_mod(ctx, path, items)?,
                None => parse_mod_in_file(ctx, path, &item.ident.to_string())?,
            },
            Item::Impl(item) => {
                let scope = match &*item.self_ty {
                    syn::Type::Path(path) => {
                        if let Some(ident) = path.path.get_ident() {
                            ident.to_string()
                        } else {
                            continue;
                        }
                    }
                    _ => continue,
                };

                parse_impl(ctx, scope, item.items)?;
            }
            _ => (),
        }
    }

    Ok(())
}

fn parse_mod_in_file(ctx: &mut Context, parent_path: &Path, name: &str) -> Result<()> {
    // Try ident.rs
    let path = parent_path
        .parent()
        .unwrap()
        .join(name)
        .with_extension("rs");

    if parse_file(ctx, &path, false)? {
        return Ok(());
    }

    // Try ident/mod.rs
    let path = parent_path.parent().unwrap().join(name).join("mod.rs");

    if parse_file(ctx, &path, false)? {
        return Ok(());
    }

    // Try self/ident.rs
    let path = parent_path
        .with_extension("")
        .join(name)
        .with_extension("rs");

    if parse_file(ctx, &path, false)? {
        return Ok(());
    }

    bail!("mod `{}` not found in `{}`", name, path.display())
}

fn parse_impl(ctx: &mut Context, scope: String, items: Vec<ImplItem>) -> Result<()> {
    for item in items {
        match item {
            ImplItem::Fn(item) => {
                if is_api_item(&item.attrs) {
                    ctx.request.variants.push(parse_request_variant(
                        scope.clone(),
                        &item.attrs,
                        &item.sig,
                    )?);
                }
            }
            _ => continue,
        }
    }

    Ok(())
}

fn is_api_item(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.meta.path().is_ident("api"))
}

fn parse_request_variant(
    scope: String,
    attrs: &[Attribute],
    sig: &Signature,
) -> Result<(String, RequestVariant)> {
    let name = sig.ident.to_string();
    let ec = || format!("failed to parse request variant `{}`", AsPascalCase(&name));

    let docs = parse_docs(attrs).with_context(ec)?;

    let mut fields = Vec::new();

    for arg in &sig.inputs {
        let arg = match arg {
            FnArg::Receiver(_) => continue,
            FnArg::Typed(arg) => arg,
        };

        let name = match &*arg.pat {
            Pat::Ident(pat) => pat.ident.to_string(),
            Pat::Wild(_) => continue,
            _ => Err(format_err!("unsupported arg type {:?}", arg.pat)).with_context(ec)?,
        };

        if ignore_request_arg(&scope, &name) {
            continue;
        }

        let ty = parse_type(&arg.ty).with_context(ec)?;

        fields.push((name, ty));
    }

    let ret = match &sig.output {
        ReturnType::Default => Type::Unit,
        ReturnType::Type(_, ty) => parse_type(ty).with_context(ec)?,
    };

    Ok((
        name,
        RequestVariant {
            docs,
            is_async: sig.asyncness.is_some(),
            fields,
            ret,
            scope,
        },
    ))
}

fn ignore_request_arg(scope: &str, name: &str) -> bool {
    scope == "Service" && (name == "conn_id" || name == "message_id")
}

fn parse_docs(attrs: &[Attribute]) -> Result<Docs> {
    let mut lines = Vec::new();

    for attr in attrs {
        match &attr.meta {
            Meta::NameValue(meta) if meta.path.is_ident("doc") => match &meta.value {
                Expr::Lit(expr) => match &expr.lit {
                    Lit::Str(lit) => {
                        lines.push(lit.value());
                    }
                    _ => bail!("invalid doc comment: {:?}", expr.lit),
                },
                _ => bail!("invalid doc comment: {:?}", meta.value),
            },
            _ => (),
        }
    }

    Ok(Docs { lines })
}

fn parse_type(ty: &syn::Type) -> Result<Type> {
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
                let arg = parse_generic_type(arg)?;
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
                        let ok = parse_generic_type(ok)?;

                        (ok, "Error".to_string())
                    }
                    2 => {
                        let ok = args.args.get(0).unwrap();
                        let ok = parse_generic_type(ok)?;

                        let err = args.args.get(1).unwrap();
                        let err = parse_generic_type(err)?;
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
                let arg = parse_generic_type(arg)?;
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
                let key = parse_generic_type(key)?;
                let key = key
                    .as_scalar()
                    .with_context(|| format!("unsupported map key type: {:?}", key))?
                    .to_owned();

                let val = args.args.get(1).unwrap();
                let val = parse_generic_type(val)?;
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

fn parse_generic_type(arg: &GenericArgument) -> Result<Type> {
    match arg {
        GenericArgument::Type(ty) => parse_type(ty),
        _ => bail!("unsupported generic argument: {:?}", arg),
    }
}
