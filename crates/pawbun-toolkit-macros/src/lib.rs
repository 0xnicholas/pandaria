#![deny(missing_docs)]
//! Procedural macros for `pawbun-toolkit`.

use proc_macro::TokenStream;
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{parse_macro_input, Ident, ItemImpl, LitStr, Token};

/// Parsed arguments for `#[pawbun_tool(...)]`.
struct ToolArgs {
    name: LitStr,
    description: LitStr,
    crate_path: syn::Path,
}

impl std::fmt::Debug for ToolArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolArgs")
            .field("name", &self.name.value())
            .field("description", &self.description.value())
            .field("crate_path", &"<syn::Path>")
            .finish()
    }
}

impl syn::parse::Parse for ToolArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let mut name = None;
        let mut description = None;
        let mut crate_path = None;

        while !input.is_empty() {
            let ident = Ident::parse_any(input)?;
            input.parse::<Token![=]>()?;

            if ident == "crate" {
                let path: syn::Path = input.parse()?;
                crate_path = Some(path);
            } else {
                let lit: LitStr = input.parse()?;
                match ident.to_string().as_str() {
                    "name" => name = Some(lit),
                    "description" => description = Some(lit),
                    other => {
                        return Err(syn::Error::new_spanned(
                            ident,
                            format!("unknown attribute: {}", other),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            name: name
                .ok_or_else(|| syn::Error::new(input.span(), "missing `name` attribute"))?,
            description: description
                .ok_or_else(|| syn::Error::new(input.span(), "missing `description` attribute"))?,
            crate_path: crate_path.unwrap_or_else(|| syn::parse_quote!(::pawbun_toolkit)),
        })
    }
}

/// 为 `impl Tool for Struct` 块自动生成样板代码。
///
/// 自动生成 `name()`、`description()` 和 `parameters()` 方法。
/// 如果 impl 块中已存在同名方法，则保留用户定义版本（需签名匹配）。
///
/// # 属性
/// - `name` — 工具名称（必填）
/// - `description` — 工具描述（必填）
/// - `crate` — 自定义 crate 路径（默认 `::pawbun_toolkit`）
///
/// # Example
/// ```ignore
/// use pawbun_toolkit::{Tool, ToolResult, ToolError};
/// use pawbun_toolkit_macros::pawbun_tool;
///
/// #[derive(Debug)]
/// struct EchoTool;
///
/// #[pawbun_tool(name = "echo", description = "Echoes the input back")]
/// impl Tool for EchoTool {
///     fn execute(&self, input: &str) -> Result<ToolResult, ToolError> {
///         Ok(ToolResult {
///             success: true,
///             content: input.into(),
///             metadata: None,
///             elapsed_ms: None,
///         })
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn pawbun_tool(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as ToolArgs);
    let mut item_impl = parse_macro_input!(input as ItemImpl);

    // 校验是否作用于 trait impl 块
    if item_impl.trait_.is_none() {
        return syn::Error::new_spanned(
            item_impl,
            "#[pawbun_tool] can only be used on `impl Trait for ...` blocks",
        )
        .into_compile_error()
        .into();
    }

    let name_lit = &args.name;
    let desc_lit = &args.description;
    let crate_path = &args.crate_path;

    // 检查 impl 块中已存在哪些方法
    let mut has_name = false;
    let mut has_description = false;
    let mut has_parameters = false;

    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            let method_name = method.sig.ident.to_string();
            match method_name.as_str() {
                "name" => {
                    if check_sig_name(&method.sig) {
                        has_name = true;
                    }
                }
                "description" => {
                    if check_sig_description(&method.sig) {
                        has_description = true;
                    }
                }
                "parameters" => {
                    if check_sig_parameters(&method.sig) {
                        has_parameters = true;
                    }
                }
                _ => {}
            }
        }
    }

    // 生成缺失的方法
    if !has_name {
        let method: syn::ImplItemFn = syn::parse_quote! {
            fn name(&self) -> &str {
                #name_lit
            }
        };
        item_impl.items.push(syn::ImplItem::Fn(method));
    }

    if !has_description {
        let method: syn::ImplItemFn = syn::parse_quote! {
            fn description(&self) -> &str {
                #desc_lit
            }
        };
        item_impl.items.push(syn::ImplItem::Fn(method));
    }

    if !has_parameters {
        let method: syn::ImplItemFn = syn::parse_quote! {
            fn parameters(&self) -> ::std::borrow::Cow<'static, [#crate_path::ToolParameter]> {
                ::std::borrow::Cow::Borrowed(&[])
            }
        };
        item_impl.items.push(syn::ImplItem::Fn(method));
    }

    let span = item_impl.span();
    TokenStream::from(quote::quote_spanned!(span => #item_impl))
}

fn check_sig_name(sig: &syn::Signature) -> bool {
    sig.generics.params.is_empty()
        && sig.inputs.len() == 1
        && matches!(sig.inputs.first(), Some(syn::FnArg::Receiver(r)) if r.reference.is_some() && r.mutability.is_none())
        && check_return_type_is_str_ref(sig)
}

fn check_return_type_is_str_ref(sig: &syn::Signature) -> bool {
    match &sig.output {
        syn::ReturnType::Default => false,
        syn::ReturnType::Type(_, ty) => is_str_ref(ty),
    }
}

fn check_sig_description(sig: &syn::Signature) -> bool {
    check_sig_name(sig) // same signature pattern
}

fn check_sig_parameters(sig: &syn::Signature) -> bool {
    sig.generics.params.is_empty()
        && sig.inputs.len() == 1
        && matches!(sig.inputs.first(), Some(syn::FnArg::Receiver(r)) if r.reference.is_some() && r.mutability.is_none())
}

fn is_str_ref(ty: &syn::Type) -> bool {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Path(p) = r.elem.as_ref() {
            return p.qself.is_none()
                && p.path.segments.len() == 1
                && p.path.segments[0].ident == "str";
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::ToTokens;

    fn parse_ts(s: &str) -> syn::Result<ToolArgs> {
        let ts: proc_macro2::TokenStream = s.parse().unwrap();
        syn::parse2(ts)
    }

    #[test]
    fn parse_valid_args() {
        let args = parse_ts(r#"name = "echo", description = "echo tool""#).unwrap();
        assert_eq!(args.name.value(), "echo");
        assert_eq!(args.description.value(), "echo tool");
    }

    #[test]
    fn parse_missing_name() {
        let result = parse_ts(r#"description = "echo tool""#);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("missing `name`"));
    }

    #[test]
    fn parse_missing_description() {
        let result = parse_ts(r#"name = "echo""#);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("missing `description`"));
    }

    #[test]
    fn parse_unknown_attr() {
        let result = parse_ts(r#"name = "echo", unknown = "x""#);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("unknown attribute"));
    }

    #[test]
    fn parse_custom_crate() {
        let args = parse_ts(r#"name = "echo", description = "echo tool", crate = ::my_crate"#).unwrap();
        assert!(args.crate_path.to_token_stream().to_string().contains("my_crate"));
    }
}
