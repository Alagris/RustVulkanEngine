use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, ItemStruct, LitStr, Path, parse_quote};

pub(crate) fn element(arguments: TokenStream, input: TokenStream) -> syn::Result<TokenStream> {
    let mut style: Path = parse_quote!(::engine::ui::css::Style);
    let mut has_style = false;
    let mut layout = false;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("style") {
            if has_style {
                return Err(meta.error("style may only be specified once"));
            }
            has_style = true;
            style = meta.value()?.parse()?;
        } else if meta.path.is_ident("layout") {
            if layout {
                return Err(meta.error("layout may only be specified once"));
            }
            layout = true;
        } else {
            return Err(meta.error("expected `style = Type` or `layout`"));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, arguments)?;
    let mut item: ItemStruct = syn::parse2(input)?;
    let Fields::Named(fields) = &mut item.fields else {
        return Err(syn::Error::new_spanned(
            item,
            "#[element] requires named fields",
        ));
    };
    if fields
        .named
        .iter()
        .any(|field| field.ident.as_ref().is_some_and(|id| id == "data"))
    {
        return Err(syn::Error::new_spanned(
            item,
            "#[element] generates `data`; remove the manually declared field",
        ));
    }
    // Keep this first so flattened fields follow the common attributes consistently.
    fields.named.insert(
        0,
        parse_quote! {
            #[serde(flatten)]
            pub data: ::engine::ui::elem::ElementData<#style>
        },
    );
    if layout {
        item.attrs.push(parse_quote!(#[ui(layout)]));
    }
    Ok(quote!(#item))
}

fn fields(
    input: &DeriveInput,
) -> syn::Result<&syn::punctuated::Punctuated<syn::Field, syn::Token![,]>> {
    if let Data::Struct(data) = &input.data
        && let Fields::Named(fields) = &data.fields
    {
        return Ok(&fields.named);
    }
    Err(syn::Error::new_spanned(
        input,
        "UI derives require a struct with named fields",
    ))
}

pub(crate) fn attributes(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mut getters = Vec::new();
    let mut setters = Vec::new();
    let mut flattened = None;
    let mut keys = std::collections::HashSet::new();
    for field in fields(input)? {
        let id = field.ident.as_ref().expect("named field");
        let mut key = id.to_string();
        let mut flatten = false;
        let mut skip = false;
        for attr in &field.attrs {
            if attr.path().is_ident("serde") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("rename") {
                        key = meta.value()?.parse::<LitStr>()?.value();
                    } else if meta.path.is_ident("flatten") {
                        flatten = true;
                    } else if meta.path.is_ident("skip") {
                        skip = true;
                    } else if meta.input.peek(syn::Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                })?;
            }
            if attr.path().is_ident("ui") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("text") {
                        skip = true;
                        Ok(())
                    } else {
                        Err(meta.error("expected `text`"))
                    }
                })?;
            }
        }
        if flatten {
            if flattened.replace(id).is_some() {
                return Err(syn::Error::new_spanned(
                    field,
                    "only one flattened attribute group is supported",
                ));
            }
        } else if !skip {
            if !keys.insert(key.clone()) {
                return Err(syn::Error::new_spanned(
                    field,
                    format!("duplicate UI attribute '{key}'"),
                ));
            }
            getters.push(quote! {
                #key => ::engine::ui::elem::AttributeValue::attribute_value(&self.#id),
            });
            setters.push(quote! {
                #key => {
                    self.#id = ::engine::ui::elem::AttributeValue::parse_attribute(value)
                        .map_err(|error| ::engine::ui::elem::anyhow::anyhow!("invalid attribute '{}': {error:#}", name))?;
                    Ok(())
                },
            });
        }
    }
    let get_fallback = flattened.map_or(
        quote!(None),
        |id| quote!(::engine::ui::elem::Attributes::attribute(&self.#id, name)),
    );
    let set_fallback = flattened.map_or(
        quote!(Err(::engine::ui::elem::anyhow::anyhow!(
            "unknown UI attribute '{}'",
            name
        ))),
        |id| quote!(::engine::ui::elem::Attributes::set_attribute(&mut self.#id, name, value)),
    );
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::engine::ui::elem::Attributes for #name #ty_generics #where_clause {
            fn attribute(&self, name: &str) -> Option<::std::borrow::Cow<'_, str>> {
                match name { #(#getters)* _ => #get_fallback }
            }
            fn set_attribute(&mut self, name: &str, value: Option<&str>) -> ::engine::ui::elem::anyhow::Result<()> {
                match name { #(#setters)* _ => #set_fallback }
            }
        }
    })
}

pub(crate) fn derive_element(input: DeriveInput) -> syn::Result<TokenStream> {
    let attributes = attributes(&input)?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mut layout = false;
    for attr in input.attrs.iter().filter(|attr| attr.path().is_ident("ui")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("layout") {
                layout = true;
                Ok(())
            } else {
                Err(meta.error("expected `layout`"))
            }
        })?;
    }
    let mut text = None;
    for field in fields(&input)? {
        for attr in field.attrs.iter().filter(|attr| attr.path().is_ident("ui")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("text") {
                    if text
                        .replace(field.ident.as_ref().expect("named field"))
                        .is_some()
                    {
                        return Err(meta.error("only one text field is supported"));
                    }
                    Ok(())
                } else {
                    Err(meta.error("expected `text`"))
                }
            })?;
        }
    }
    if !fields(&input)?
        .iter()
        .any(|field| field.ident.as_ref().is_some_and(|id| id == "data"))
    {
        return Err(syn::Error::new_spanned(
            &input,
            "Element requires #[element(...)] or a flattened `data` field",
        ));
    }
    let text_methods = text.map(|id| {
        quote! {
            fn text(&self) -> Option<&str> { Some(&self.#id) }
            fn set_text(&mut self, text: String) -> ::engine::ui::elem::anyhow::Result<()> {
                self.#id = text;
                Ok(())
            }
        }
    });
    let layout_impl = layout.then(|| {
        quote! {
            #[automatically_derived]
            impl #impl_generics ::engine::ui::elem::Layout for #name #ty_generics #where_clause {}
        }
    });
    Ok(quote! {
        #attributes
        #[automatically_derived]
        impl #impl_generics ::engine::ui::elem::Element for #name #ty_generics #where_clause {
            fn tag(&self) -> &str { stringify!(#name) }
            fn data(&self) -> &dyn ::engine::ui::elem::Attributes { &self.data }
            fn inline_style(&self) -> ::engine::ui::css::Style {
                ::engine::ui::elem::UiStyle::to_style(&self.data.style)
            }
            fn validate_style(&self, style: &::engine::ui::css::Style) -> ::engine::ui::elem::anyhow::Result<()> {
                ::engine::ui::elem::validate_style(&self.data.style, style)
            }
            fn is_layout(&self) -> bool { #layout }
            #text_methods
        }
        #layout_impl
    })
}

pub(crate) fn style(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input,
            "UiStyle requires a concrete style type",
        ));
    }
    let ids = fields(&input)?
        .iter()
        .map(|field| field.ident.as_ref().expect("named field"))
        .collect::<Vec<&Ident>>();
    let style_fields = [
        "background",
        "foreground",
        "text_align",
        "font_size",
        "left",
        "right",
        "top",
        "bottom",
    ];
    for id in &ids {
        if !style_fields.iter().any(|field| id == field) {
            return Err(syn::Error::new_spanned(id, "unsupported UI style field"));
        }
    }
    let initializers = style_fields.iter().map(|field| {
        let id = Ident::new(field, name.span());
        if ids.contains(&&id) {
            quote!(#id: self.#id.clone())
        } else {
            quote!(#id: None)
        }
    });
    let mutable = (!ids.is_empty()).then(|| quote!(mut));
    Ok(quote! {
        #[automatically_derived]
        impl ::engine::ui::elem::UiStyle for #name {
            fn to_style(&self) -> ::engine::ui::css::Style {
                ::engine::ui::css::Style {
                    #(#initializers,)*
                }
            }
            fn from_style(#mutable style: ::engine::ui::css::Style) -> ::engine::ui::elem::anyhow::Result<Self> {
                let result = Self { #(#ids: style.#ids.take()),* };
                if !style.is_empty() {
                    return Err(::engine::ui::elem::anyhow::anyhow!("unsupported style properties for '{}': {:?}", stringify!(#name), style));
                }
                Ok(result)
            }
        }
        #[automatically_derived]
        impl ::engine::ui::elem::AttributeValue for #name {
            fn attribute_value(&self) -> Option<::std::borrow::Cow<'_, str>> {
                ::engine::ui::elem::style_attribute(self)
            }
            fn parse_attribute(value: Option<&str>) -> ::engine::ui::elem::anyhow::Result<Self> {
                ::engine::ui::elem::parse_style_attribute(value)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_element_shapes_and_options_are_rejected() {
        assert!(
            element(
                quote!(layout, layout),
                quote!(
                    struct Invalid {}
                )
            )
            .is_err()
        );
        assert!(
            element(
                quote!(style = A, style = B),
                quote!(
                    struct Invalid {}
                )
            )
            .is_err()
        );
        assert!(
            attributes(&syn::parse_quote!(
                struct Duplicate {
                    #[serde(rename = "same")]
                    first: String,
                    #[serde(rename = "same")]
                    second: String,
                }
            ))
            .is_err()
        );
        assert!(
            style(syn::parse_quote!(
                struct Generic<T> {
                    value: T,
                }
            ))
            .is_err()
        );
        assert!(
            style(syn::parse_quote!(
                struct Tuple(String);
            ))
            .is_err()
        );
        assert!(
            element(
                quote!(),
                quote!(
                    struct Invalid;
                )
            )
            .is_err()
        );
        assert!(
            element(
                quote!(),
                quote!(
                    struct Invalid {
                        data: String,
                    }
                )
            )
            .is_err()
        );
        assert!(
            element(
                quote!(unknown),
                quote!(
                    struct Invalid {}
                )
            )
            .is_err()
        );
        assert!(
            derive_element(syn::parse_quote!(
                struct Missing {}
            ))
            .is_err()
        );
        assert!(
            attributes(&syn::parse_quote!(
                enum Invalid {
                    One,
                }
            ))
            .is_err()
        );
        assert!(
            attributes(&syn::parse_quote!(
                struct Invalid {
                    #[serde(flatten)]
                    a: String,
                    #[serde(flatten)]
                    b: String,
                }
            ))
            .is_err()
        );
    }

    #[test]
    fn injection_precedes_serde_derives_and_uses_requested_style() {
        let output = element(
            quote!(style = CustomStyle, layout),
            quote! {
                #[derive(Deserialize, Element)]
                struct Panel { title: Option<String> }
            },
        )
        .unwrap();
        let item: ItemStruct = syn::parse2(output).unwrap();
        let Fields::Named(fields) = item.fields else {
            panic!("expected named fields")
        };
        assert_eq!(fields.named.len(), 2);
        assert_eq!(fields.named[0].ident.as_ref().unwrap(), "data");
        let ty = &fields.named[0].ty;
        assert!(quote!(#ty).to_string().contains("CustomStyle"));
    }
}
