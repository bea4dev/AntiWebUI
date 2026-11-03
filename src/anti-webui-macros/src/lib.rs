//! Derive macros for AntiWebUI. Use them through the `anti-webui` crate.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Path, parse_macro_input};

/// Generates the WGSL struct of a shader's parameters and how to write them, so that the two can't
/// disagree on the layout. See `docs/design/14-shaders.md`.
///
/// Fields can be `f32`, `i32`, `u32`, `[f32; 2]`, `[f32; 3]`, `[f32; 4]`, `Color` and
/// `[[f32; 4]; N]`. Inside of AntiWebUI itself, where `anti_webui` doesn't exist, say where the
/// shader module is with `#[shader_params(crate = "anti_webui_core")]`.
#[proc_macro_derive(ShaderParams, attributes(shader_params))]
pub fn derive_shader_params(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "shader parameters can't be generic",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "shader parameters must be a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "shader parameters must have named fields",
        ));
    };
    let mut krate: Path = syn::parse_quote!(::anti_webui::core);
    for attr in &input.attrs {
        if attr.path().is_ident("shader_params") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    krate = meta.value()?.parse::<syn::LitStr>()?.parse()?;
                    Ok(())
                } else {
                    Err(meta.error("expected `crate`"))
                }
            })?;
        }
    }
    let shader = quote!(#krate::shader);
    let idents: Vec<_> = fields.named.iter().map(|f| &f.ident).collect();
    let names: Vec<_> = idents
        .iter()
        .map(|i| i.as_ref().map(ToString::to_string))
        .collect();
    let types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();
    Ok(quote! {
        impl #shader::ShaderParams for #name {
            const FIELDS: &'static [#shader::ParamField] = &[
                #(#shader::ParamField {
                    name: #names,
                    ty: <#types as #shader::ParamValue>::TYPE,
                },)*
            ];

            fn write(&self, out: &mut #shader::ParamWriter) {
                #(#shader::ParamValue::write(&self.#idents, out);)*
            }
        }
    })
}
