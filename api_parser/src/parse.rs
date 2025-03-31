use std::{fs, io, path::Path};

use anyhow::{bail, format_err, Context as _, Result};
use heck::AsPascalCase;
use syn::{
    parenthesized, punctuated::Punctuated, Attribute, BinOp, Expr, ExprBinary, FnArg,
    GenericArgument, ImplItem, ItemEnum, ItemStruct, Lit, Meta, Pat, PathArguments, ReturnType,
    Signature, Token,
};

use crate::{
    ComplexEnum, ComplexVariant, Context, Docs, EnumRepr, Field, Item, Newtype, RequestVariant,
    SimpleEnum, SimpleVariant, Struct, Type,
};

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

fn parse_mod(ctx: &mut Context, path: &Path, items: Vec<syn::Item>) -> Result<()> {
    for item in items {
        match item {
            syn::Item::Enum(item) => {
                if !is_api_item(&item.attrs) {
                    continue;
                }

                let name = item.ident.to_string();
                ctx.items.push((
                    name.clone(),
                    parse_enum(item).with_context(|| {
                        format!("failed to parse enum {name} in {}", path.display())
                    })?,
                ))
            }
            syn::Item::Struct(item) => {
                if !is_api_item(&item.attrs) {
                    continue;
                }

                let name = item.ident.to_string();
                ctx.items.push((
                    name.clone(),
                    parse_struct(item).with_context(|| {
                        format!("failed to parse struct {name} in {}", path.display())
                    })?,
                ));
            }
            syn::Item::Mod(item) => match item.content {
                Some((_, items)) => parse_mod(ctx, path, items)?,
                None => parse_mod_in_file(ctx, path, &item.ident.to_string())?,
            },
            syn::Item::Impl(item) => {
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

enum Variant {
    Simple(SimpleVariant),
    Complex(ComplexVariant),
}

fn parse_enum(item: ItemEnum) -> Result<Item> {
    if item.generics.lt_token.is_some() {
        bail!("generic enums not supported");
    }

    let docs = parse_docs(&item.attrs)?;
    let repr = parse_enum_repr(&item.attrs)?;

    let mut next_value = 0;
    let mut variants = Vec::new();

    for variant in item.variants {
        let docs = parse_docs(&variant.attrs)?;
        let name = variant.ident.to_string();

        let variant = match variant.fields {
            syn::Fields::Named(fields) => {
                let fields = parse_named_fields(&fields.named)?;
                Variant::Complex(ComplexVariant { docs, fields })
            }
            syn::Fields::Unnamed(fields) => {
                let fields = parse_unnamed_fields(&fields.unnamed)?;

                if fields.len() != 1 {
                    bail!("enum variants with more than one unnamed fields not supported");
                }

                let fields = fields
                    .into_iter()
                    .take(1)
                    .map(|field| ("value".to_owned(), field))
                    .collect();

                Variant::Complex(ComplexVariant { docs, fields })
            }
            syn::Fields::Unit => {
                let value = if let Some((_, expr)) = variant.discriminant {
                    parse_const_int_expr(expr)?
                } else {
                    next_value
                };

                next_value = value + 1;

                Variant::Simple(SimpleVariant { docs, value })
            }
        };

        variants.push((name, variant));
    }

    if variants
        .iter()
        .any(|(_, variant)| matches!(variant, Variant::Complex(_)))
    {
        // convert all variants to complex
        let variants = variants
            .into_iter()
            .map(|(name, variant)| {
                (
                    name,
                    match variant {
                        Variant::Simple(v) => into_complex_variant(v),
                        Variant::Complex(v) => v,
                    },
                )
            })
            .collect();

        Ok(Item::ComplexEnum(ComplexEnum { docs, variants }))
    } else {
        let variants: Vec<_> = variants
            .into_iter()
            .map(|(name, variant)| {
                (
                    name,
                    match variant {
                        Variant::Simple(v) => v,
                        Variant::Complex(_) => unreachable!(),
                    },
                )
            })
            .collect();

        let repr = repr.unwrap_or_else(|| infer_enum_repr(&variants));

        Ok(Item::SimpleEnum(SimpleEnum {
            docs,
            repr,
            variants,
        }))
    }
}

fn parse_struct(item: ItemStruct) -> Result<Item> {
    if item.generics.lt_token.is_some() {
        bail!("generic structs not supported");
    }

    let docs = parse_docs(&item.attrs)?;

    let ty = if let Some(ty) = parse_api_repr(&item.attrs)? {
        Some(Some(ty))
    } else if is_serde_transparent(&item.attrs) {
        Some(None)
    } else {
        None
    };

    if let Some(ty) = ty {
        if item.fields.len() != 1 {
            bail!("transparent structs must have exactly one field");
        }

        if let Some(ty) = ty {
            Ok(Item::Newtype(Newtype { docs, ty }))
        } else {
            let field = item.fields.iter().next().unwrap();
            let ty = parse_type(&field.ty)?;

            Ok(Item::Newtype(Newtype { docs, ty }))
        }
    } else {
        match item.fields {
            syn::Fields::Named(fields) => {
                let fields = parse_named_fields(&fields.named)?;
                Ok(Item::Struct(Struct { docs, fields }))
            }
            syn::Fields::Unnamed(_) => bail!("non-transparent tuple structs not supported"),
            syn::Fields::Unit => bail!("unit structs not supported"),
        }
    }
}

fn parse_enum_repr(attrs: &[Attribute]) -> Result<Option<EnumRepr>> {
    for attr in attrs {
        let Meta::List(meta) = &attr.meta else {
            continue;
        };

        if !meta.path.is_ident("repr") {
            continue;
        }

        return Ok(Some(meta.tokens.to_string().parse()?));
    }

    Ok(None)
}

fn infer_enum_repr(variants: &[(String, SimpleVariant)]) -> EnumRepr {
    let max = variants.iter().map(|(_, v)| v.value).max().unwrap_or(0);

    if max <= u8::MAX as u64 {
        EnumRepr::U8
    } else if max <= u16::MAX as u64 {
        EnumRepr::U16
    } else if max <= u32::MAX as u64 {
        EnumRepr::U32
    } else {
        EnumRepr::U64
    }
}

fn parse_named_fields(fields: &Punctuated<syn::Field, Token![,]>) -> Result<Vec<(String, Field)>> {
    let mut out = Vec::new();

    for field in fields {
        let name = if let Some(ident) = &field.ident {
            ident.to_string()
        } else {
            bail!("unnamed fields not supported");
        };

        let docs = parse_docs(&field.attrs)?;
        let ty = parse_type(&field.ty)?;
        let field = Field { docs, ty };

        out.push((name, field));
    }

    Ok(out)
}

fn parse_unnamed_fields(fields: &Punctuated<syn::Field, Token![,]>) -> Result<Vec<Field>> {
    let mut out = Vec::new();

    for field in fields {
        let docs = parse_docs(&field.attrs)?;
        let ty = parse_type(&field.ty)?;
        let field = Field { docs, ty };

        out.push(field);
    }

    Ok(out)
}

fn parse_api_repr(attrs: &[Attribute]) -> Result<Option<Type>> {
    for attr in attrs {
        if !attr.path().is_ident("api") {
            continue;
        }

        let list = match &attr.meta {
            Meta::List(list) => list,
            _ => continue,
        };

        let mut repr_ty = None;

        list.parse_nested_meta(|meta| {
            if !meta.path.is_ident("repr") {
                return Ok(());
            }

            if meta.input.peek(syn::token::Paren) {
                let content;
                parenthesized!(content in meta.input);
                let ty: syn::Type = content.parse()?;
                repr_ty = Some(ty);
            }

            Ok(())
        })?;

        if let Some(ty) = repr_ty {
            return Ok(Some(parse_type(&ty)?));
        }
    }

    Ok(None)
}

fn is_serde_transparent(attrs: &[Attribute]) -> bool {
    for attr in attrs {
        let list = match &attr.meta {
            Meta::List(list) => list,
            _ => continue,
        };

        if !list.path.is_ident("serde") {
            continue;
        }

        let ident: syn::Ident = if let Ok(ident) = list.parse_args() {
            ident
        } else {
            continue;
        };

        if ident == "transparent" {
            return true;
        }
    }

    false
}

fn into_complex_variant(v: SimpleVariant) -> ComplexVariant {
    ComplexVariant {
        docs: v.docs,
        fields: Vec::new(),
    }
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
        PathArguments::None => {
            let name = first.ident.to_string();

            if name == "Bytes" {
                Ok(Type::Bytes)
            } else {
                Ok(Type::Scalar(name))
            }
        }
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

fn parse_const_int_expr(expr: Expr) -> Result<u64> {
    match expr {
        Expr::Lit(expr) => parse_int_lit(expr.lit),
        Expr::Binary(expr) => parse_const_int_binary_expr(expr),
        _ => bail!("unsupported integer expr: {:?}", expr),
    }
}

fn parse_int_lit(lit: Lit) -> Result<u64> {
    match lit {
        Lit::Int(lit) => {
            if let Ok(value) = lit.base10_parse() {
                Ok(value)
            } else {
                bail!("int literal overflow: {}", lit.base10_digits());
            }
        }
        Lit::Byte(lit) => Ok(lit.value() as _),
        _ => {
            bail!("not an int or byte literal: {:?}", lit);
        }
    }
}

fn parse_const_int_binary_expr(expr: ExprBinary) -> Result<u64> {
    let lhs = parse_const_int_expr(*expr.left)?;
    let rhs = parse_const_int_expr(*expr.right)?;

    match expr.op {
        BinOp::Add(_) => Ok(lhs + rhs),
        BinOp::Sub(_) => Ok(lhs - rhs),
        BinOp::Mul(_) => Ok(lhs * rhs),
        BinOp::Div(_) => Ok(lhs / rhs),
        BinOp::Rem(_) => Ok(lhs % rhs),
        BinOp::BitXor(_) => Ok(lhs ^ rhs),
        BinOp::BitAnd(_) => Ok(lhs & rhs),
        BinOp::BitOr(_) => Ok(lhs & rhs),
        BinOp::Shl(_) => Ok(lhs << rhs),
        BinOp::Shr(_) => Ok(lhs >> rhs),
        _ => bail!("unsupported binary op: {:?}", expr.op),
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test_parse_simple_enum() {
        let item: syn::ItemEnum = parse_quote! {
            #[api]
            enum Color {
                Red,
                Green,
                Blue,
            }
        };
        assert_matches!(
            parse_enum(item),
            Ok(Item::SimpleEnum(item)) => {
                assert_eq!(item.variants.len(), 3);
                assert_eq!(item.variants[0].0, "Red");
                assert_eq!(item.variants[1].0, "Green");
                assert_eq!(item.variants[2].0, "Blue");
            }
        );
    }

    #[test]
    fn test_parse_newtype() {
        let item: syn::ItemStruct = parse_quote! {
            #[derive(Serialize)]
            #[api]
            #[serde(transparent)]
            struct Name(String);
        };
        assert_matches!(
            parse_struct(item),
            Ok(Item::Newtype(Newtype { ty, .. })) => {
                assert_matches!(
                    ty,
                    Type::Scalar(name) => assert_eq!(name, "String")
                )
            }
        );

        let item: syn::ItemStruct = parse_quote! {
            #[api(repr(String))]
            struct Password(Box<Zeroizing<String>>);
        };
        assert_matches!(
            parse_struct(item),
            Ok(Item::Newtype(Newtype { ty, .. })) => {
                assert_matches!(
                    ty,
                    Type::Scalar(name) => assert_eq!(name, "String")
                )
            }
        );

        let item: syn::ItemStruct = parse_quote! {
            #[derive(Serialize)]
            #[serde(transparent)]
            #[api(repr(Bytes))]
            pub struct PublicRuntimeId {
                public: PublicKey,
            }
        };
        assert_matches!(
            parse_struct(item),
            Ok(Item::Newtype(Newtype { ty, .. })) => {
                assert_matches!(ty, Type::Bytes)
            }
        );
    }

    #[test]
    fn test_parse_api_repr() {
        let attrs: Vec<Attribute> = parse_quote! {
            #[api]
        };
        assert_matches!(parse_api_repr(&attrs), Ok(None));

        let attrs: Vec<Attribute> = parse_quote! {
            #[api(nonsense)]
        };
        assert_matches!(parse_api_repr(&attrs), Ok(None));

        let attrs: Vec<Attribute> = parse_quote! {
            #[api(repr(u32))]
        };
        assert_matches!(
            parse_api_repr(&attrs),
            Ok(Some(Type::Scalar(name))) if name == "u32"
        );
    }
}
