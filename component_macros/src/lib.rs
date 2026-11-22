use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, ItemStruct, Meta, Path, Token, parse_quote,
};

#[proc_macro_attribute]
pub fn component(_arguments: TokenStream, input: TokenStream) -> TokenStream {
    let mut item = parse_macro_input!(input as ItemStruct);
    let Fields::Named(fields) = &mut item.fields else {
        return syn::Error::new_spanned(item, "#[component] requires a struct with named fields")
            .to_compile_error()
            .into();
    };
    if fields
        .named
        .iter()
        .any(|field| field.ident.as_ref().is_some_and(|name| name == "instance"))
    {
        return syn::Error::new_spanned(
            item,
            "#[component] adds the `instance` field; remove the manually declared field",
        )
        .to_compile_error()
        .into();
    }

    let mut attributes: Vec<Attribute> = Vec::new();
    if derives_serde(&item.attrs) {
        attributes.push(parse_quote!(#[serde(skip)]));
    }
    let instance_field = parse_quote! {
        #(#attributes)*
        instance: ::engine::ecs::component::ComponentInstance
    };
    fields.named.push(instance_field);
    quote!(#item).into()
}

#[proc_macro_derive(Component, attributes(require, unique))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_component(input) {
        Ok(expansion) => expansion.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_component(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    let Data::Struct(data) = input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "Component can only be derived for structs",
        ));
    };
    let Fields::Named(fields) = data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "Component requires a struct with named fields",
        ));
    };
    if !fields.named.iter().any(|field| {
        field
            .ident
            .as_ref()
            .is_some_and(|field| field == "instance")
    }) {
        return Err(syn::Error::new_spanned(
            name,
            "Component requires an `instance: ComponentInstance` field; add #[component] to inject it",
        ));
    }

    let requirements = input
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("require"))
        .map(parse_requirements)
        .collect::<syn::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let unique = input
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("unique"))
        .try_fold(false, |found, attribute| {
            if found {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "a component may only have one #[unique] attribute",
                ));
            }
            if !matches!(attribute.meta, Meta::Path(_)) {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "#[unique] does not accept arguments",
                ));
            }
            Ok(true)
        })?;

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let requirement_values = requirements.iter().map(|path| quote!(stringify!(#path)));
    Ok(quote! {
        impl #impl_generics ::std::ops::Deref for #name #ty_generics #where_clause {
            type Target = ::engine::ecs::component::ComponentInstance;

            fn deref(&self) -> &Self::Target {
                &self.instance
            }
        }

        impl #impl_generics ::std::ops::DerefMut for #name #ty_generics #where_clause {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.instance
            }
        }

        impl #impl_generics ::engine::ecs::component::ComponentMetadata
            for #name #ty_generics #where_clause
        {
            fn required_components() -> &'static [&'static str] {
                &[#(#requirement_values),*]
            }

            fn is_unique() -> bool {
                #unique
            }
        }
    })
}

fn parse_requirements(attribute: &Attribute) -> syn::Result<Vec<Path>> {
    let requirements =
        attribute.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
    if requirements.is_empty() {
        return Err(syn::Error::new_spanned(
            attribute,
            "#[require] needs at least one component type",
        ));
    }
    Ok(requirements.into_iter().collect())
}

fn derives_serde(attributes: &[Attribute]) -> bool {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("derive"))
        .filter_map(|attribute| {
            attribute
                .parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
                .ok()
        })
        .flatten()
        .any(|path| {
            path.segments.last().is_some_and(|segment| {
                segment.ident == Ident::new("Serialize", segment.ident.span())
                    || segment.ident == Ident::new("Deserialize", segment.ident.span())
            })
        })
}
