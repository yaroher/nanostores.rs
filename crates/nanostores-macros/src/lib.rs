use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, GenericParam, LitStr, Type, parse_macro_input, parse_quote};

#[proc_macro_derive(NanoMap, attributes(serde))]
pub fn derive_nano_map(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_nano_map(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_nano_map(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let rename_all = container_rename_all(&input.attrs)?;
    let fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "NanoMap can only be derived for structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "NanoMap can only be derived for structs",
            ));
        }
    };

    let mut generics = input.generics;
    let field_types = fields
        .iter()
        .map(|field| field.ty.clone())
        .collect::<Vec<_>>();
    add_field_bounds(&mut generics, &field_types);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let mut keys = Vec::new();
    let mut set_arms = Vec::new();
    let mut get_arms = Vec::new();
    let mut setters = Vec::new();
    let mut setter_impls = Vec::new();

    for field in fields {
        let syn::Field {
            attrs, ident, ty, ..
        } = field;
        let ident = ident.expect("named fields always have identifiers");
        let key = field_rename(&attrs)?
            .unwrap_or_else(|| rename_field(&ident.to_string(), rename_all.as_deref()));
        let key_lit = LitStr::new(&key, ident.span());
        let setter = format_ident!("set_{}", ident);

        keys.push(quote!(#key_lit));
        set_arms.push(quote! {
            #key_lit => {
                self.#ident = <#ty as ::serde::Deserialize>::deserialize(value)
                    .map_err(|error| ::nanostores::KeyError::deserialize(#key_lit, error))?;
                Ok(())
            }
        });
        get_arms.push(quote! {
            #key_lit => {
                ::serde::Serialize::serialize(&self.#ident, ser)
                    .map_err(|error| ::nanostores::KeyError::serialize(#key_lit, error))
            }
        });
        setters.push(quote! {
            fn #setter(&self, value: #ty);
        });
        setter_impls.push(quote! {
            fn #setter(&self, value: #ty) {
                self.set_field_by(#key_lit, move |state| {
                    if state.#ident == value {
                        false
                    } else {
                        state.#ident = value;
                        true
                    }
                });
            }
        });
    }

    let ext_trait = format_ident!("{}NanoMapExt", name);

    Ok(quote! {
        impl #impl_generics ::nanostores::NanoMap for #name #ty_generics #where_clause {
            const KEYS: &'static [&'static str] = &[#(#keys),*];

            fn set_field<'de, __NanoD>(
                &mut self,
                key: &str,
                value: __NanoD,
            ) -> ::std::result::Result<(), ::nanostores::KeyError>
            where
                __NanoD: ::serde::Deserializer<'de>,
            {
                match key {
                    #(#set_arms,)*
                    _ => Err(::nanostores::KeyError::unknown(key)),
                }
            }

            fn get_field<__NanoS>(
                &self,
                key: &str,
                ser: __NanoS,
            ) -> ::std::result::Result<__NanoS::Ok, ::nanostores::KeyError>
            where
                __NanoS: ::serde::Serializer,
            {
                match key {
                    #(#get_arms,)*
                    _ => Err(::nanostores::KeyError::unknown(key)),
                }
            }
        }

        pub trait #ext_trait #impl_generics #where_clause {
            #(#setters)*
        }

        impl #impl_generics #ext_trait #ty_generics for ::nanostores::MapStore<#name #ty_generics>
        #where_clause
        {
            #(#setter_impls)*
        }
    })
}

fn add_field_bounds(generics: &mut syn::Generics, field_types: &[Type]) {
    for param in generics.params.iter_mut() {
        if let GenericParam::Type(param) = param {
            param.bounds.push(parse_quote!(::std::cmp::PartialEq));
            param.bounds.push(parse_quote!(::std::clone::Clone));
            param.bounds.push(parse_quote!(::std::marker::Send));
            param.bounds.push(parse_quote!(::std::marker::Sync));
            param.bounds.push(parse_quote!('static));
        }
    }

    let where_clause = generics.make_where_clause();
    for ty in field_types {
        where_clause
            .predicates
            .push(parse_quote!(#ty: ::serde::Serialize));
        where_clause
            .predicates
            .push(parse_quote!(#ty: for<'__nano_de> ::serde::Deserialize<'__nano_de>));
        where_clause
            .predicates
            .push(parse_quote!(#ty: ::std::cmp::PartialEq));
    }
}

fn container_rename_all(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    let mut rename_all = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                let value: LitStr = meta.value()?.parse()?;
                rename_all = Some(value.value());
                Ok(())
            } else {
                skip_meta_value(&meta)
            }
        })?;
    }
    Ok(rename_all)
}

fn field_rename(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    let mut rename = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value: LitStr = meta.value()?.parse()?;
                rename = Some(value.value());
                Ok(())
            } else {
                skip_meta_value(&meta)
            }
        })?;
    }
    Ok(rename)
}

/// Consume the value of an unrelated serde meta (`default = "path"`,
/// `bound(deserialize = "...")`, bare `skip`) so `parse_nested_meta` does not
/// error out on attributes this derive does not care about.
fn skip_meta_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let value = meta.value()?;
        value.parse::<syn::Lit>()?;
    } else if meta.input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in meta.input);
        content.parse::<proc_macro2::TokenStream>()?;
    }
    Ok(())
}

fn rename_field(name: &str, rename_all: Option<&str>) -> String {
    // Mirrors serde's RenameRule applied to snake_case field identifiers.
    match rename_all {
        Some("lowercase") => name.to_lowercase(),
        Some("UPPERCASE") | Some("SCREAMING_SNAKE_CASE") => name.to_uppercase(),
        Some("PascalCase") => to_pascal_case(name),
        Some("camelCase") => to_camel_case(name),
        Some("kebab-case") => name.replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => name.to_uppercase().replace('_', "-"),
        _ => name.to_owned(),
    }
}

fn to_pascal_case(name: &str) -> String {
    let mut out = String::new();
    for part in name.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn to_camel_case(name: &str) -> String {
    let mut parts = name.split('_');
    let mut out = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}
