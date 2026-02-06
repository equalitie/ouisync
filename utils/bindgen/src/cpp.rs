use anyhow::Result;
use heck::{AsPascalCase, AsShoutySnakeCase, AsSnakeCase};
use ouisync_api_parser::{
    ComplexEnum, Context, Docs, EnumRepr, Fields, Item, RequestVariant, SimpleEnum, Struct,
    ToResponseVariantName, Type,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    fs::File,
    io::{self, Write},
    path::Path,
};

struct OutFiles<'a> {
    hpp: &'a mut dyn Write,
    cpp: &'a mut dyn Write,
    dsc: &'a mut dyn Write,
}

#[derive(Debug, Clone, Copy)]
enum MessageType {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy)]
enum ItemType {
    Data,
    Message(MessageType),
}

impl ItemType {
    fn is_message(&self) -> bool {
        match self {
            Self::Data => false,
            Self::Message(_) => true,
        }
    }
}

//#pub(crate) fn generate(ctx: &Context, out: &mut dyn Write) -> Result<()> {
pub(crate) fn generate(ctx: &Context, out_dir: &Path) -> Result<()> {
    const DO_NOT_EDIT_MESSAGE: &str = "// This file is auto generated. Do not edit.";

    let request_enum = ctx.request.to_enum();
    let response_enum = ctx.response.to_enum();

    let mut data_items: Vec<NamedItem> = ctx
        .items
        .iter()
        .map(|(name, item)| match item {
            Item::SimpleEnum(item) => NamedItem::SimpleEnum {
                item_type: ItemType::Data,
                name,
                item,
            },
            Item::ComplexEnum(item) => NamedItem::ComplexEnum {
                item_type: ItemType::Data,
                name,
                item,
            },
            Item::Struct(item) => NamedItem::Struct {
                item_type: ItemType::Data,
                name,
                item,
            },
        })
        .chain([
            NamedItem::ComplexEnum {
                item_type: ItemType::Message(MessageType::Request),
                name: "Request",
                item: &request_enum,
            },
            NamedItem::ComplexEnum {
                item_type: ItemType::Message(MessageType::Response),
                name: "Response",
                item: &response_enum,
            },
        ])
        .collect();

    let mut api_items: Vec<ApiClass> = [
        ApiClass {
            name: "Session",
            inner: None,
            variants: &ctx.request.variants,
        },
        ApiClass {
            name: "Repository",
            inner: Some(("handle", "RepositoryHandle")),
            variants: &ctx.request.variants,
        },
        ApiClass {
            name: "File",
            inner: Some(("handle", "FileHandle")),
            variants: &ctx.request.variants,
        },
    ]
    .into_iter()
    .collect();

    reorder(&mut data_items);
    reorder(&mut api_items);

    let mut data_hpp_file = File::create(out_dir.join("data.g.hpp"))?;
    let data_hpp = &mut data_hpp_file;

    let mut data_cpp_file = File::create(out_dir.join("data.g.cpp"))?;
    let data_cpp = &mut data_cpp_file;

    let mut data_dsc_file = File::create(out_dir.join("data_dsc.g.hpp"))?;
    let data_dsc = &mut data_dsc_file;

    let mut msg_hpp_file = File::create(out_dir.join("message.g.hpp"))?;
    let msg_hpp = &mut msg_hpp_file;

    let mut msg_cpp_file = File::create(out_dir.join("message.g.cpp"))?;
    let msg_cpp = &mut msg_cpp_file;

    let mut msg_dsc_file = File::create(out_dir.join("message_dsc.g.hpp"))?;
    let msg_dsc = &mut msg_dsc_file;

    writeln!(data_hpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(data_hpp)?;
    writeln!(data_hpp, "#pragma once")?;
    writeln!(data_hpp)?;
    writeln!(data_hpp, "#include <cstdint>")?;
    writeln!(data_hpp, "#include <list>")?;
    writeln!(data_hpp, "#include <optional>")?;
    writeln!(data_hpp, "#include <memory>")?;
    writeln!(data_hpp, "#include <sstream>")?;
    writeln!(data_hpp, "#include <chrono>")?;
    writeln!(data_hpp, "#include <variant>")?;
    writeln!(data_hpp)?;
    writeln!(data_hpp, "namespace {NAMESPACE} {{")?;
    writeln!(data_hpp)?;

    writeln!(data_dsc, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(data_dsc)?;
    writeln!(data_dsc, "#pragma once")?;
    writeln!(data_dsc)?;
    writeln!(data_dsc, "#include <ouisync/describe.hpp>")?;
    writeln!(data_dsc)?;
    writeln!(data_dsc, "namespace {NAMESPACE} {{")?;
    writeln!(data_dsc)?;
    writeln!(
        data_dsc,
        "template<typename Variant> struct VariantBuilder;"
    )?;
    writeln!(data_hpp)?;

    writeln!(data_cpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(data_cpp)?;
    writeln!(data_cpp, "#include <ouisync/data.hpp>")?;
    writeln!(data_cpp)?;
    writeln!(data_cpp, "namespace {NAMESPACE} {{")?;
    writeln!(data_cpp)?;

    writeln!(msg_hpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(msg_hpp)?;
    writeln!(msg_hpp, "#pragma once")?;
    writeln!(msg_hpp)?;
    writeln!(msg_hpp, "#include <string_view>")?;
    writeln!(msg_hpp)?;
    writeln!(msg_hpp, "namespace {NAMESPACE} {{")?;
    writeln!(msg_hpp)?;

    writeln!(msg_cpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(msg_cpp)?;
    writeln!(msg_cpp, "#include <ouisync/data.hpp>")?;
    writeln!(msg_cpp, "#include <ouisync/message.g.hpp>")?;
    writeln!(msg_cpp)?;
    writeln!(msg_cpp, "namespace {NAMESPACE} {{")?;
    writeln!(msg_cpp)?;

    writeln!(msg_dsc, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(msg_dsc)?;
    writeln!(msg_dsc, "#pragma once")?;
    writeln!(msg_dsc)?;
    writeln!(msg_dsc, "#include <ouisync/describe.hpp>")?;
    writeln!(msg_dsc)?;
    writeln!(msg_dsc, "namespace {NAMESPACE} {{")?;
    writeln!(msg_dsc)?;

    for item in &data_items {
        let mut out = if item.item_type().is_message() {
            OutFiles {
                hpp: msg_hpp,
                cpp: msg_cpp,
                dsc: msg_dsc,
            }
        } else {
            OutFiles {
                hpp: data_hpp,
                cpp: data_cpp,
                dsc: data_dsc,
            }
        };

        item.render(&mut out)?;
    }

    writeln!(data_hpp, "}} // namespace {NAMESPACE}")?;
    writeln!(data_cpp, "}} // namespace {NAMESPACE}")?;
    writeln!(data_dsc, "}} // namespace {NAMESPACE}")?;

    writeln!(msg_hpp, "}} // namespace {NAMESPACE}")?;
    writeln!(msg_cpp, "}} // namespace {NAMESPACE}")?;
    writeln!(msg_dsc, "}} // namespace {NAMESPACE}")?;

    let mut hpp_file = File::create(out_dir.join("api.g.hpp"))?;
    let out_hpp = &mut hpp_file;

    let mut cpp_file = File::create(out_dir.join("api.g.cpp"))?;
    let out_cpp = &mut cpp_file;

    writeln!(out_hpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(out_hpp)?;
    writeln!(out_hpp, "#include <ouisync/data.hpp>")?;
    writeln!(out_hpp, "#include <boost/asio/spawn.hpp>")?;
    writeln!(out_hpp, "#include <boost/filesystem/path.hpp>")?;
    writeln!(out_hpp)?;
    writeln!(out_hpp, "namespace {NAMESPACE} {{")?;
    writeln!(out_hpp)?;
    writeln!(out_hpp, "class Client;")?;
    writeln!(out_hpp)?;

    writeln!(out_cpp, "{}", DO_NOT_EDIT_MESSAGE)?;
    writeln!(out_cpp)?;
    writeln!(out_cpp, "#include <ouisync/client.hpp>")?;
    writeln!(out_cpp, "#include <ouisync/api.g.hpp>")?;
    writeln!(out_cpp, "#include <ouisync/message.g.hpp>")?;
    writeln!(out_cpp)?;
    writeln!(out_cpp, "namespace {NAMESPACE} {{")?;
    writeln!(out_cpp)?;

    for item in &api_items {
        write_api_class(out_hpp, out_cpp, item.name, item.inner, item.variants)?;
    }

    writeln!(out_hpp, "}} // namespace {NAMESPACE}")?;
    writeln!(out_cpp, "}} // namespace {NAMESPACE}")?;
    Ok(())
}

// Reorder items such that if item A uses item B, then B will be before A.
fn reorder(items: &mut Vec<impl DependentItem>) {
    let mut seen: HashSet<String> = HashSet::new();

    let relevant: HashSet<_> = items.iter().map(|item| item.name().to_string()).collect();

    for i in 0..items.len() {
        let mut rendered: bool = false;

        for j in i..items.len() {
            // TODO: Sub optimal `.collect` because some maps have `String` and some have
            // `&String`.
            let uses: HashSet<_> = items[j].depends_on().iter().cloned().collect();
            let dependencies: HashSet<_> = uses.intersection(&relevant).cloned().collect();
            let can_render = dependencies.difference(&seen).next().is_none();

            if can_render {
                if i != j {
                    items.swap(i, j);
                }
                rendered = true;
                seen.insert(items[i].name().into());
                break;
            }
        }

        if !rendered {
            panic!("Iteration did not write any item");
        };
    }
}

fn write_simple_enum(out: &mut OutFiles<'_>, name: &str, item: &SimpleEnum) -> Result<()> {
    let repr = match item.repr {
        EnumRepr::U8 => "uint8_t",
        EnumRepr::U16 => "uint16_t",
        EnumRepr::U32 => "uint32_t",
        EnumRepr::U64 => "uint64_t",
    };

    let (name, is_error) = if name == "ErrorCode" {
        ("Service", true)
    } else {
        (name, false)
    };

    if is_error {
        writeln!(out.hpp, "namespace error {{")?;
    }

    write_docs(out.hpp, "", &item.docs)?;
    writeln!(out.hpp, "enum {name} : {repr} {{")?;

    for (variant_name, variant) in &item.variants {
        write_docs(out.hpp, I, &variant.docs)?;
        writeln!(
            out.hpp,
            "{I}{} = {},",
            AsShoutySnakeCase(variant_name),
            variant.value
        )?;
    }

    writeln!(out.hpp, "}};")?;

    if is_error {
        writeln!(out.hpp, "}} // namespace error")?;
    }

    writeln!(out.hpp)?;

    if is_error {
        write_service_error_code(name, out.hpp, out.cpp, item)?;
    }

    writeln!(out.hpp)?;

    #[cfg_attr(any(), rustfmt::skip)]
    {
        let namespace_prefix = if is_error {
            "error::"
        } else {
            ""
        };

        writeln!(out.dsc, "template<> struct describe::Enum<{namespace_prefix}{name}> : std::true_type {{")?;
        writeln!(out.dsc, "{I}static bool is_valid({repr} v) {{")?;
        writeln!(out.dsc, "{I}{I}static const {repr} values[] = {{")?;
        for (variant_name, _variant) in &item.variants {
            writeln!(out.dsc, "{I}{I}{I}{namespace_prefix}{},", AsShoutySnakeCase(variant_name),)?;
        }
        writeln!(out.dsc, "{I}{I}}};")?;
        writeln!(out.dsc, "{I}{I}for (size_t i = 0; i != {}; ++i) {{", item.variants.len())?;
        writeln!(out.dsc, "{I}{I}{I}if (v == values[i]) return true;")?;
        writeln!(out.dsc, "{I}{I}}}")?;
        writeln!(out.dsc, "{I}{I}return false;")?;
        writeln!(out.dsc, "{I}}}")?;
        writeln!(out.dsc, "}};")?;
        writeln!(out.dsc)?;
    }

    Ok(())
}

enum FieldsParseType {
    Array,
    Direct,
}

impl FieldsParseType {
    fn new(fields: &Fields) -> Self {
        match fields {
            Fields::Named(_) => Self::Array,
            Fields::Unnamed(_) => Self::Direct,
            Fields::Unit => Self::Direct,
        }
    }
}

#[rustfmt::skip]
fn describe_struct<'a>(
    out: &mut dyn Write,
    name: &str,
    base_type: Option<&str>,
    parse_type: FieldsParseType,
    fields: impl Iterator<Item = &'a str> + Clone,
) -> Result<()> {
    let fields_type = match parse_type {
        FieldsParseType::Array => "ARRAY",
        FieldsParseType::Direct => "DIRECT",
    };

    let empty = base_type.is_none() && fields.clone().count() == 0;
    writeln!(out, "template<> struct describe::Struct<{name}> : std::true_type {{")?;
    writeln!(out, "{I}static const describe::FieldsType fields_type = describe::FieldsType::{fields_type};")?;
    writeln!(out, "{I}template<class Observer>")?;
    if !empty {
        writeln!(out, "{I}static void describe(Observer& o, {name}& v) {{")?;
    } else {
        // Prevent "unused variable" warnings.
        writeln!(out, "{I}static void describe(Observer&, {name}&) {{")?;
    }
    if let Some(base_type) = base_type {
        writeln!(out, "{I}{I}o.field(static_cast<{base_type}&>(v));")?;
    }
    for field_name in fields {
        writeln!(out, "{I}{I}o.field(v.{});", AsSnakeCase(field_name))?;
    }
    writeln!(out, "{I}}}")?;
    writeln!(out, "}};")?;
    Ok(())
}

fn write_complex_enum(
    out: &mut OutFiles<'_>,
    name: &str,
    item: &ComplexEnum,
    item_type: ItemType,
) -> Result<()> {
    let is_request = match item_type {
        ItemType::Data => false,
        ItemType::Message(MessageType::Request) => true,
        ItemType::Message(MessageType::Response) => false,
    };

    write_docs(out.hpp, "", &item.docs)?;

    writeln!(out.hpp, "struct {name} {{")?;

    // variants
    for (variant_name, variant) in &item.variants {
        write_docs(out.hpp, I, &variant.docs)?;
        writeln!(out.hpp, "{I}struct {variant_name} {{")?;

        // Member variables
        for (name, field) in &variant.fields {
            let ty = CppType::new(&field.ty);
            let ty = ty.modify(is_request, true);
            let name = AsSnakeCase(name.unwrap_or(DEFAULT_FIELD_NAME));
            writeln!(out.hpp, "{I}{I}{} {};", &ty, name)?;
        }

        writeln!(out.hpp, "{I}}};")?;
        writeln!(out.hpp)?;

        describe_struct(
            out.dsc,
            &format!("{name}::{variant_name}"),
            None,
            FieldsParseType::new(&variant.fields),
            variant.fields.default_named(DEFAULT_FIELD_NAME).names(),
        )?;
        writeln!(out.dsc)?;
    }

    writeln!(out.hpp, "{I}using {ALTERNATIVES_SUFFIX} = std::variant<")?;
    for (i, (variant_name, _variant)) in item.variants.iter().enumerate() {
        write!(out.hpp, "{I}{I}{variant_name}")?;
        if i != item.variants.len() - 1 {
            writeln!(out.hpp, ",")?;
        } else {
            writeln!(out.hpp)?;
        }
    }
    writeln!(out.hpp, "{I}>;")?;

    writeln!(out.hpp)?;
    writeln!(out.hpp, "{I}{ALTERNATIVES_SUFFIX} value;")?;
    writeln!(out.hpp)?;

    writeln!(out.hpp, "{I}{name}() = default;")?;
    writeln!(out.hpp, "{I}{name}({name}&&) = default;")?;
    writeln!(out.hpp, "{I}{name}& operator=({name}&&) = default;")?;
    // TODO: Implement explicit cloning
    writeln!(out.hpp, "{I}{name}({name} const&) = default;")?;
    writeln!(out.hpp)?;
    writeln!(out.hpp, "{I}template<class T>")?;
    writeln!(out.hpp, "{I}{name}(T&& v)")?;
    writeln!(out.hpp, "{I}{I}: value(std::forward<T>(v)) {{}}")?;
    writeln!(out.hpp)?;
    writeln!(out.hpp, "{I}template<class T>")?;
    writeln!(out.hpp, "{I}T& get() {{")?;
    writeln!(out.hpp, "{I}{I}return std::get<T>(value);")?;
    writeln!(out.hpp, "{I}}}")?;
    writeln!(out.hpp)?;
    writeln!(out.hpp, "{I}template<class T>")?;
    writeln!(out.hpp, "{I}T* get_if() {{")?;
    writeln!(out.hpp, "{I}{I}return std::get_if<T>(&value);")?;
    writeln!(out.hpp, "{I}}}")?;
    writeln!(out.hpp)?;
    writeln!(out.hpp, "}};")?;

    describe_struct(
        out.dsc,
        name,
        None,
        FieldsParseType::Direct,
        ["value"].into_iter(),
    )?;
    writeln!(out.dsc)?;

    #[cfg_attr(any(), rustfmt::skip)]
    {
        writeln!(out.dsc, "template<> struct VariantBuilder<{name}::{ALTERNATIVES_SUFFIX}> {{")?;
        writeln!(out.dsc, "{I}template<class AltBuilder>")?;
        writeln!(out.dsc, "{I}static {name}::Alternatives build(std::string_view name, const AltBuilder& builder) {{")?;
        for (variant_name, _variant) in item.variants.iter() {
            writeln!(out.dsc, "{I}{I}if (name == \"{variant_name}\") {{")?;
            writeln!(out.dsc, "{I}{I}{I}return builder.template build<{name}::{variant_name}>();")?;
            writeln!(out.dsc, "{I}{I}}}")?;
        }
        writeln!(out.dsc)?;
        writeln!(out.dsc, "{I}{I}throw std::runtime_error(\"invalid variant name for {name}\");")?;
        writeln!(out.dsc, "{I}}}")?;
        writeln!(out.dsc, "}};")?;
        writeln!(out.dsc)?;
    }

    #[cfg_attr(any(), rustfmt::skip)]
    {
        writeln!(out.dsc, "std::string_view variant_name(const {name}::{ALTERNATIVES_SUFFIX}& variant);")?;
        writeln!(out.dsc)?;

        writeln!(out.cpp, "std::string_view variant_name(const {name}::{ALTERNATIVES_SUFFIX}& variant) {{")?;
        for (variant_name, _variant) in item.variants.iter() {
            writeln!(out.cpp, "{I}if (std::get_if<{name}::{variant_name}>(&variant) != nullptr) {{")?;
            writeln!(out.cpp, "{I}{I}return std::string_view(\"{variant_name}\");")?;
            writeln!(out.cpp, "{I}}}")?;
        }
        writeln!(out.cpp, "{I}throw std::bad_cast();")?;
        writeln!(out.cpp, "}}")?;
        writeln!(out.cpp)?;
    }

    Ok(())
}

fn write_struct(out: &mut OutFiles<'_>, name: &str, item: &Struct) -> Result<()> {
    write_docs(out.hpp, "", &item.docs)?;

    writeln!(out.hpp, "struct {name} {{")?;
    write!(out.hpp, "{}", DefineVariables::new(1, &item.fields))?;

    writeln!(out.hpp, "}};")?; // struct end
    writeln!(out.hpp)?;

    describe_struct(
        out.dsc,
        name,
        None,
        FieldsParseType::new(&item.fields),
        item.fields.default_named(DEFAULT_FIELD_NAME).names(),
    )?;

    writeln!(out.dsc)?;

    Ok(())
}

fn write_service_error_code(
    name: &str,
    hpp: &mut dyn Write,
    cpp: &mut dyn Write,
    item: &SimpleEnum,
) -> Result<()> {
    writeln!(hpp, "const char* error_message(error::{name} ec) noexcept;")?;
    writeln!(hpp)?;

    writeln!(
        cpp,
        "const char* error_message(error::{name} ec) noexcept {{"
    )?;
    writeln!(cpp, "{I}switch (ec) {{")?;
    for (variant_name, _variant) in &item.variants {
        writeln!(
            cpp,
            "{I}{I}case error::{}: return \"{variant_name}\";",
            AsShoutySnakeCase(variant_name)
        )?;
    }
    writeln!(cpp, "{I}{I}default: return \"UnknownError\";")?;
    writeln!(cpp, "{I}}}")?;
    writeln!(cpp, "}}")?;
    writeln!(cpp)?;

    Ok(())
}

fn write_api_class(
    out_hpp: &mut dyn Write,
    out_cpp: &mut dyn Write,
    name: &str,
    inner: Option<(&str, &str)>,
    request_variants: &[(String, RequestVariant)],
) -> Result<()> {
    writeln!(out_hpp, "class {name} {{")?;
    writeln!(out_hpp, "private:")?;

    for friend in ["File", "Repository", "Session", "RepositorySubscription"]
        .into_iter()
        .filter(|friend| *friend != name)
    {
        writeln!(out_hpp, "{I}friend class {friend};")?;
    }
    writeln!(out_hpp)?;

    // Members
    writeln!(out_hpp, "{I}std::shared_ptr<Client> client;")?;

    if let Some((name, ty)) = inner {
        writeln!(out_hpp, "{I}{ty} {name};")?;
    }
    writeln!(out_hpp)?;

    // constructor
    write!(out_hpp, "{I}explicit {name}(std::shared_ptr<Client> client")?;

    if let Some((name, ty)) = inner {
        write!(out_hpp, ", {ty} {name}")?;
    }

    writeln!(out_hpp, ") :")?;
    write!(out_hpp, "{I}{I}client(std::move(client))")?;

    if let Some((name, _ty)) = inner {
        writeln!(out_hpp, ",")?;
        writeln!(out_hpp, "{I}{I}{name}(std::move({name}))")?;
    } else {
        writeln!(out_hpp)?;
    }

    writeln!(out_hpp, "{I}{{}}")?;

    writeln!(out_hpp)?;
    writeln!(out_hpp, "public:")?;

    if name == "Session" {
        let docs_lines = [
            "Connect to the Ouisync service and return Session on success. Throws on failure.",
            "",
            "config_dir_path: Path to a directory created by Ouisync service",
            "                 containing local_endpoint.conf file.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

        write_docs(out_hpp, I, &Docs { lines: docs_lines })?;
        writeln!(out_hpp, "{I}static Session connect(")?;
        #[rustfmt::skip]
        writeln!(out_hpp, "{I}{I}const boost::filesystem::path& config_dir_path,")?;
        writeln!(out_hpp, "{I}{I}boost::asio::yield_context")?;
        writeln!(out_hpp, "{I});")?;
        writeln!(out_hpp)?;

        writeln!(out_cpp, "Session Session::connect(")?;
        #[rustfmt::skip]
        writeln!(out_cpp, "{I}const boost::filesystem::path& config_dir,")?;
        writeln!(out_cpp, "{I}boost::asio::yield_context yield")?;
        writeln!(out_cpp, ") {{")?;
        writeln!(
            out_cpp,
            "{I}auto client = ouisync::Client::connect(config_dir, yield);"
        )?;
        writeln!(
            out_cpp,
            "{I}return Session(std::make_shared<Client>(std::move(client)));"
        )?;
        writeln!(out_cpp, "}}")?;
        writeln!(out_cpp)?;
    }

    let prefix = format!("{}_", AsSnakeCase(name));

    for (variant_name, variant) in request_variants {
        // Event subscription / unsubscription is handled manually
        if variant_name.contains("subscribe") {
            continue;
        }

        let Some(op_name) = variant_name.strip_prefix(&prefix) else {
            continue;
        };

        // Return type
        let ret = match &variant.ret {
            Type::Result(ty, _) => ty,
            ty => ty,
        };
        // Return type with the "Handle" suffix stripped. Useful for mapping handles to their target
        // types (e.g., `RepositoryHandle` -> `Repository`).
        let ret_stripped = ret.strip_suffix("Handle");
        let response_variant_name = format!("Response::{}", ret.to_response_variant_name());

        write_docs(out_hpp, I, &variant.docs)?;

        let ret_cpp_type = ret_stripped
            .as_ref()
            .map(CppType::new)
            .unwrap_or(CppType::new(ret));

        declare_function(
            out_hpp,
            Indent(1),
            &ret_cpp_type,
            None,
            op_name,
            &variant.fields,
            inner.is_some(),
        )?;

        writeln!(out_hpp, ";")?;
        writeln!(out_hpp)?;

        declare_function(
            out_cpp,
            Indent(0),
            &ret_cpp_type,
            Some(name),
            op_name,
            &variant.fields,
            inner.is_some(),
        )?;

        writeln!(out_cpp, " {{")?;

        // request
        write!(
            out_cpp,
            "{I}auto request = Request::{}",
            AsPascalCase(variant_name)
        )?;

        if !variant.fields.is_empty() {
            writeln!(out_cpp, "{{")?;

            let inner_name = inner.map(|(name, _)| AsSnakeCase(name));

            for (index, (arg_name, _)) in variant.fields.iter().enumerate() {
                let arg_name = AsSnakeCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME));

                if index == 0
                    && let Some(inner_name) = &inner_name {
                        writeln!(out_cpp, "{I}{I}{inner_name},")?;
                        continue;
                    }

                writeln!(out_cpp, "{I}{I}{arg_name},")?;
            }

            writeln!(out_cpp, "{I}}};")?;
        } else {
            writeln!(out_cpp, "();")?;
        }

        // response
        writeln!(
            out_cpp,
            "{I}// TODO: This won't throw if yield has ec assigned"
        )?;
        writeln!(
            out_cpp,
            "{I}auto response = client->invoke(request, yield);"
        )?;

        match ret {
            Type::Unit => writeln!(out_cpp, "{I}response.get<Response::None>();")?,
            Type::Option(_) => {
                writeln!(
                    out_cpp,
                    "{I}if (response.get_if<{response_variant_name}>() == nullptr) return {{}};",
                )?;

                writeln!(
                    out_cpp,
                    "{I}return std::move(response.get<{response_variant_name}>()).value;"
                )?;
            }
            _ => {
                writeln!(
                    out_cpp,
                    "{I}{response_variant_name} rsp = std::move(response.get<{response_variant_name}>());"
                )?;
                match ret_stripped {
                    Some(Type::Scalar(w)) => {
                        writeln!(out_cpp, "{I}return {w}(client, std::move(rsp.value));")?;
                    }
                    //Some(Type::Vec(w)) => write!(out_cpp, "response.value.map {{ {w}(client, it) }}")?,
                    Some(Type::Vec(ty)) => {
                        writeln!(out_cpp, "{I}std::vector<{}> vec;", CppScalar::new(&ty))?;
                        writeln!(out_cpp, "{I}vec.reserve(rsp.value.size());")?;
                        writeln!(out_cpp, "{I}for (auto v : rsp.value) {{")?;
                        writeln!(
                            out_cpp,
                            "{I}{I}vec.emplace_back({ty}(client, std::move(v)))"
                        )?;
                        writeln!(out_cpp, "{I}}}")?;
                        writeln!(out_cpp, "{I}return vec;")?;
                    }
                    Some(Type::Map(k_ty, v_ty)) => {
                        writeln!(
                            out_cpp,
                            "{I}std::map<{}, {v_ty}> map;",
                            CppScalar::new(&k_ty)
                        )?;
                        writeln!(out_cpp, "{I}for (auto [k, v] : rsp.value) {{")?;
                        writeln!(
                            out_cpp,
                            "{I}{I}map.emplace(std::make_pair(std::move(k), {v_ty}(client, std::move(v))));"
                        )?;
                        writeln!(out_cpp, "{I}}}")?;
                        writeln!(out_cpp, "{I}return map;")?;
                    }
                    Some(_) => unreachable!(),
                    None => writeln!(out_cpp, "{I}return std::move(rsp.value);")?,
                }
            }
        }

        writeln!(out_cpp, "}}")?;
        writeln!(out_cpp)?;
    }

    // TODO
    //// equals + hashCode + toString
    //if let Some((inner_name, _)) = inner {
    //    writeln!(out_hpp, "{I}override fun equals(other: Any?): Boolean =")?;
    //    writeln!(
    //        out_hpp,
    //        "{I}{I}other is {name} && client == other.client && {inner_name} == other.{inner_name}"
    //    )?;

    //    writeln!(out_hpp)?;
    //    writeln!(
    //        out_hpp,
    //        "{I}override fun hashCode(): Int = Objects.hash(client, {inner_name})"
    //    )?;

    //    writeln!(out_hpp)?;
    //    writeln!(
    //        out_hpp,
    //        "{I}override fun toString(): String = \"${{this::class.simpleName}}(${inner_name})\""
    //    )?;
    //}

    writeln!(out_hpp, "}};")?;
    writeln!(out_hpp)?;

    Ok(())
}

fn declare_function(
    out: &mut dyn Write,
    indent: Indent,
    ret_type: &CppType,
    class_prefix: Option<&str>,
    op_name: &str,
    fields: &Fields,
    has_inner: bool,
) -> io::Result<()> {
    // These are C++ keywords, so can't be used as function names
    let op_rename: HashMap<&str, &str> = [
        ("delete", "delete_repository"),
        ("export", "export_repository"),
    ]
    .into_iter()
    .collect();

    // Use default argument only if there is at least one other non-default argument.
    let use_default_args = fields
        .iter()
        .skip(if has_inner { 1 } else { 0 })
        .any(|(_, field)| CppType::new(&field.ty).default().is_none());

    writeln!(
        out,
        "{indent}{} {}{}(",
        ret_type,
        if let Some(class) = class_prefix {
            format!("{class}::")
        } else {
            "".into()
        },
        AsSnakeCase(*op_rename.get(&op_name).unwrap_or(&op_name))
    )?;

    let mut added_yield = false;

    for (index, (arg_name, field)) in fields.iter().enumerate() {
        if index == 0 && has_inner {
            continue;
        }

        let is_last = index == fields.len() - 1;

        let ty = CppType::new(&field.ty);

        if use_default_args && ty.default().is_some() && !added_yield {
            writeln!(out, "{indent}{I}boost::asio::yield_context yield,")?;
            added_yield = true;
        }

        write!(
            out,
            "{indent}{I}{} {}",
            &ty.modify(true, false),
            AsSnakeCase(arg_name.unwrap_or(DEFAULT_FIELD_NAME)),
        )?;

        if use_default_args && class_prefix.is_none()
            && let Some(default) = ty.default() {
                write!(out, " = {default}")?;
            }

        if !is_last || !added_yield {
            writeln!(out, ",")?;
        } else {
            writeln!(out)?;
        }
    }

    if !added_yield {
        writeln!(out, "{indent}{I}boost::asio::yield_context yield")?;
    }

    write!(out, "{indent})")
}

fn write_docs(out: &mut dyn Write, prefix: &str, docs: &Docs) -> io::Result<()> {
    if docs.lines.is_empty() {
        return Ok(());
    }

    writeln!(out, "{prefix}/**")?;

    for line in &docs.lines {
        writeln!(out, "{prefix} *{line}")?;
    }

    writeln!(out, "{prefix} */")?;

    Ok(())
}

#[derive(Debug, Clone)]
struct ApiClass<'a> {
    name: &'a str,
    inner: Option<(&'a str, &'a str)>,
    variants: &'a Vec<(String, RequestVariant)>,
}

#[derive(Debug, Clone)]
enum NamedItem<'a> {
    SimpleEnum {
        item_type: ItemType,
        name: &'a str,
        item: &'a SimpleEnum,
    },
    ComplexEnum {
        item_type: ItemType,
        name: &'a str,
        item: &'a ComplexEnum,
    },
    Struct {
        item_type: ItemType,
        name: &'a str,
        item: &'a Struct,
    },
}

impl<'a> NamedItem<'a> {
    fn name(&self) -> &'a str {
        match self {
            Self::SimpleEnum { name, .. } => name,
            Self::ComplexEnum { name, .. } => name,
            Self::Struct { name, .. } => name,
        }
    }

    fn item_type(&self) -> ItemType {
        match self {
            Self::SimpleEnum { item_type, .. } => *item_type,
            Self::ComplexEnum { item_type, .. } => *item_type,
            Self::Struct { item_type, .. } => *item_type,
        }
    }

    fn render(&self, out: &mut OutFiles<'_>) -> Result<()> {
        match self {
            Self::SimpleEnum {
                name,
                item,
                item_type: _,
            } => {
                write_simple_enum(out, name, item)?;
            }
            Self::ComplexEnum {
                name,
                item,
                item_type,
            } => {
                write_complex_enum(out, name, item, *item_type)?;
            }
            Self::Struct {
                name,
                item,
                item_type: _,
            } => {
                write_struct(out, name, item)?;
            }
        }
        Ok(())
    }
}

trait DependentItem {
    fn name(&self) -> &str;
    // List Ouisync types that this item uses (depends on).
    fn depends_on(&self) -> HashSet<String>;
}

impl<'a> DependentItem for NamedItem<'a> {
    fn name(&self) -> &str {
        self.name()
    }

    fn depends_on(&self) -> HashSet<String> {
        match self {
            Self::SimpleEnum { .. } => Default::default(),
            Self::ComplexEnum { item, .. } => item
                .variants
                .iter()
                .flat_map(|(_name, variant)| {
                    variant
                        .fields
                        .iter()
                        .map(|(_name, field)| &field.ty)
                        .flat_map(decompose)
                })
                .collect(),
            Self::Struct { item, .. } => item
                .fields
                .iter()
                .flat_map(|(_name, field)| decompose(&field.ty))
                .collect(),
        }
    }
}

impl<'a> DependentItem for ApiClass<'a> {
    fn name(&self) -> &str {
        self.name
    }

    fn depends_on(&self) -> HashSet<String> {
        let prefix = format!("{}_", AsSnakeCase(self.name));
        self.variants
            .iter()
            .filter(|(name, _variant)| name.strip_prefix(&prefix).is_some())
            .flat_map(|(_name, variant)| decompose(&variant.ret))
            .map(|ret_type| ret_type.strip_suffix("Handle").unwrap_or(&ret_type).into())
            .filter(|ty_name: &String| !ty_name.contains("subscribe"))
            .collect()
    }
}

#[derive(Clone, Copy)]
enum CppType<'a> {
    Scalar(CppScalar<'a>),
    Option(CppScalar<'a>),
    Vec(CppScalar<'a>),
    Map(CppScalar<'a>, CppScalar<'a>),
    Modified {
        const_ref: bool,
        namespaced: bool,
        inner: &'a CppType<'a>,
    },
}

impl<'a> CppType<'a> {
    fn new(ty: &'a Type) -> Self {
        match ty {
            Type::Unit => CppType::Scalar(CppScalar::Unit),
            Type::Scalar(ty_str) => CppType::Scalar(CppScalar::new(ty_str)),
            Type::Option(ty_str) => CppType::Option(CppScalar::new(ty_str)),
            Type::Result(_ty, _ty_str) => unreachable!(),
            Type::Vec(ty_str) => CppType::Vec(CppScalar::new(ty_str)),
            Type::Map(ty1_str, ty2_str) => {
                CppType::Map(CppScalar::new(ty1_str), CppScalar::new(ty2_str))
            }
            Type::Bytes => CppType::Scalar(CppScalar::Bytes),
        }
    }

    fn default(&self) -> Option<&str> {
        match self {
            Self::Option(_) => Some("{}"),
            Self::Scalar(CppScalar::Bool) => Some("false"),
            _ => None,
        }
    }

    fn modify(&'a self, const_ref: bool, namespaced: bool) -> CppType<'a> {
        match self {
            Self::Modified { inner, .. } => Self::Modified {
                const_ref,
                namespaced,
                inner,
            },
            _ => Self::Modified {
                const_ref,
                namespaced,
                inner: self,
            },
        }
    }
}

impl fmt::Display for CppType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalar(s) => write!(f, "{}", s),
            Self::Option(s) => write!(f, "std::optional<{}>", s),
            Self::Vec(s) => write!(f, "std::vector<{}>", s),
            Self::Map(k, v) => write!(f, "std::map<{}, {}>", k, v),
            Self::Modified {
                const_ref,
                namespaced,
                inner,
            } => match *inner {
                Self::Scalar(s) => {
                    if *const_ref && s.is_copy_expensive() {
                        write!(f, "const {}&", s.modify(*namespaced))
                    } else {
                        write!(f, "{}", s.modify(*namespaced))
                    }
                }
                Self::Option(s) => {
                    if !const_ref {
                        write!(f, "std::optional<{}>", s.modify(*namespaced))
                    } else {
                        write!(f, "const std::optional<{}>&", s.modify(*namespaced))
                    }
                }
                Self::Vec(s) => {
                    if !const_ref {
                        write!(f, "std::vector<{}>", s.modify(*namespaced))
                    } else {
                        write!(f, "const std::vector<{}>&", s.modify(*namespaced))
                    }
                }
                Self::Map(s1, s2) => {
                    if !const_ref {
                        write!(
                            f,
                            "std::map<{}, {}>",
                            s1.modify(*namespaced),
                            s2.modify(*namespaced)
                        )
                    } else {
                        write!(
                            f,
                            "const std::map<{}, {}>&",
                            s2.modify(*namespaced),
                            s2.modify(*namespaced)
                        )
                    }
                }
                Self::Modified { .. } => unreachable!(),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum CppScalar<'a> {
    Unit,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    Usize,
    Bool,
    Duration,
    SystemTime,
    String,
    Bytes,
    Local(&'a str),
    Modified {
        namespaced: bool,
        inner: &'a CppScalar<'a>,
    },
}

impl<'a> CppScalar<'a> {
    fn new(ty: &'a str) -> Self {
        match ty {
            "u8" => Self::U8,
            "i8" => Self::I8,
            "u16" => Self::U16,
            "i16" => Self::I16,
            "u32" => Self::U32,
            "i32" => Self::I32,
            "u64" => Self::U64,
            "i64" => Self::I64,
            "usize" => Self::Usize,
            "isize" => todo!(),
            "bool" => Self::Bool,
            "Duration" => Self::Duration,
            "SystemTime" => Self::SystemTime,
            "PathBuf" | "PeerAddr" | "SocketAddr" | "String" => Self::String,
            "StateMonitor" => Self::Local("StateMonitorNode"),
            _ => Self::Local(ty),
        }
    }

    fn modify(&'a self, namespaced: bool) -> CppScalar<'a> {
        match self {
            Self::Modified { inner, .. } => Self::Modified { namespaced, inner },
            _ => Self::Modified {
                namespaced,
                inner: self,
            },
        }
    }

    fn is_copy_expensive(&self) -> bool {
        match self {
            Self::Unit => false,
            Self::U8 => false,
            Self::I8 => false,
            Self::U16 => false,
            Self::I16 => false,
            Self::U32 => false,
            Self::I32 => false,
            Self::U64 => false,
            Self::I64 => false,
            Self::Usize => false,
            Self::Bool => false,
            Self::Duration => false,
            Self::SystemTime => false,
            Self::String => true,
            Self::Bytes => true,
            Self::Local(_) => true,
            Self::Modified { inner, .. } => inner.is_copy_expensive(),
        }
    }
}

impl fmt::Display for CppScalar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit => write!(f, "void"),
            Self::U8 => write!(f, "unt8_t"),
            Self::I8 => write!(f, "int8_t"),
            Self::U16 => write!(f, "uint16_t"),
            Self::I16 => write!(f, "int16_t"),
            Self::U32 => write!(f, "unt32_t"),
            Self::I32 => write!(f, "int32_t"),
            Self::U64 => write!(f, "uint64_t"),
            Self::I64 => write!(f, "int64_t"),
            Self::Usize => write!(f, "size_t"),
            Self::Bool => write!(f, "bool"),
            Self::Duration => write!(f, "std::chrono::milliseconds"),
            Self::SystemTime => write!(f, "std::chrono::time_point<std::chrono::system_clock>"),
            Self::String => write!(f, "std::string"),
            Self::Bytes => write!(f, "std::vector<uint8_t>"),
            Self::Local(v) => write!(f, "{v}"),
            Self::Modified { namespaced, inner } => {
                if !namespaced {
                    write!(f, "{inner}")
                } else {
                    match inner {
                        Self::Local(v) => write!(f, "ouisync::{v}"),
                        _ => write!(f, "{inner}"),
                    }
                }
            }
        }
    }
}

const NAMESPACE: &str = "ouisync";
const I: &str = "    ";
const DEFAULT_FIELD_NAME: &str = "value";
const ALTERNATIVES_SUFFIX: &str = "Alternatives";

fn decompose(ty: &Type) -> HashSet<String> {
    match ty {
        Type::Unit => Default::default(),
        Type::Scalar(ty_str) => [ty_str.clone()].into_iter().collect(),
        Type::Option(ty_str) => [ty_str.clone()].into_iter().collect(),
        Type::Result(ty, ty_str) => decompose(ty).iter().chain([ty_str]).cloned().collect(),
        Type::Vec(ty_str) => [ty_str.clone()].into_iter().collect(),
        Type::Map(ty1_str, ty2_str) => [ty1_str.clone(), ty2_str.clone()].into_iter().collect(),
        Type::Bytes => Default::default(),
    }
}

// fmt::Display utilities

struct Indent(u8);

impl fmt::Display for Indent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.0 {
            write!(f, "{}", I)?;
        }
        Ok(())
    }
}

struct DefineVariables<'a> {
    indent: u8,
    fields: &'a Fields,
}

impl<'a> DefineVariables<'a> {
    fn new(indent: u8, fields: &'a Fields) -> Self {
        Self { indent, fields }
    }
}

impl fmt::Display for DefineVariables<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, field) in self.fields {
            writeln!(
                f,
                "{}{} {};",
                Indent(self.indent),
                CppType::new(&field.ty),
                AsSnakeCase(name.unwrap_or(DEFAULT_FIELD_NAME))
            )?;
        }
        Ok(())
    }
}
