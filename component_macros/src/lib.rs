use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::parse_macro_input;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Data, DeriveInput, Fields, Ident, ItemStruct, LitStr, Meta, Path, Token, parse_quote,
};

#[proc_macro_attribute]
pub fn post_process(arguments: TokenStream, input: TokenStream) -> TokenStream {
    let type_name = parse_macro_input!(arguments as LitStr);
    if type_name.value().trim().is_empty() {
        return syn::Error::new_spanned(type_name, "postprocess type name must not be empty")
            .to_compile_error()
            .into();
    }
    let item = parse_macro_input!(input as ItemStruct);
    if !item.generics.params.is_empty() {
        return syn::Error::new_spanned(
            item,
            "postprocess registration does not support generic structs",
        )
        .to_compile_error()
        .into();
    }
    let name = &item.ident;
    quote! {
        #item

        ::engine::postprocess::inventory::submit! {
            ::engine::postprocess::PostProcessRegistration {
                type_name: #type_name,
                factory: |data| {
                    use ::engine::postprocess::PostProcessPass as _;
                    let pass: #name =
                        ::engine::postprocess::serde_yaml::from_value(data.clone())?;
                    pass.validate()?;
                    Ok(Box::new(pass).into_stages())
                },
            }
        }
    }
    .into()
}

#[proc_macro_attribute]
pub fn component(arguments: TokenStream, input: TokenStream) -> TokenStream {
    let arguments = match syn::parse::<ComponentMacroArguments>(arguments) {
        Ok(arguments) => arguments,
        Err(error) => return error.to_compile_error().into(),
    };
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
    if arguments.serialize_with.is_some() || arguments.deserialize_with.is_some() {
        let mut factory_options = Vec::new();
        if let Some(path) = arguments.serialize_with {
            factory_options.push(quote!(serialize_with = #path));
        }
        if let Some(path) = arguments.deserialize_with {
            factory_options.push(quote!(deserialize_with = #path));
        }
        let factory_attribute = parse_quote! {
            #[component_factory(#(#factory_options),*)]
        };
        item.attrs.push(factory_attribute);
    }
    let instance_field = parse_quote! {
        #(#attributes)*
        instance: ::engine::ecs::component::ComponentInstance
    };
    fields.named.push(instance_field);
    quote!(#item).into()
}

struct ComponentMacroArguments {
    serialize_with: Option<Path>,
    deserialize_with: Option<Path>,
}

impl Parse for ComponentMacroArguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut arguments = Self {
            serialize_with: None,
            deserialize_with: None,
        };
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let path: Path = input.parse()?;
            let target = if key == "serialize_with" {
                &mut arguments.serialize_with
            } else if key == "deserialize_with" {
                &mut arguments.deserialize_with
            } else {
                return Err(syn::Error::new_spanned(
                    key,
                    "expected `serialize_with` or `deserialize_with`",
                ));
            };
            if target.replace(path).is_some() {
                return Err(syn::Error::new_spanned(
                    key,
                    "component serde hooks may only be specified once",
                ));
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(arguments)
    }
}

#[proc_macro_derive(Component, attributes(require, unique, component_factory))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_component(input) {
        Ok(expansion) => expansion.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(DataAsset, attributes(data_asset))]
pub fn derive_data_asset(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_data_asset(input) {
        Ok(expansion) => expansion.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(Modifier, attributes(modifier))]
pub fn derive_modifier(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_modifier(input) {
        Ok(expansion) => expansion.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_modifier(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    if !matches!(input.data, Data::Struct(_)) {
        return Err(syn::Error::new_spanned(
            name,
            "Modifier can only be derived for structs",
        ));
    }
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "Modifier cannot be derived for generic types",
        ));
    }
    let mut compile_with = None;
    let mut input_type = None;
    let mut output_type = None;
    for attribute in input
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("modifier"))
    {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("compile_with")
                || meta.path.is_ident("input_type")
                || meta.path.is_ident("output_type")
            {
                let path: Path = meta.value()?.parse()?;
                let slot = if meta.path.is_ident("compile_with") {
                    &mut compile_with
                } else if meta.path.is_ident("input_type") {
                    &mut input_type
                } else {
                    &mut output_type
                };
                if slot.replace(path).is_some() {
                    return Err(meta.error("modifier attribute fields may only be specified once"));
                }
                Ok(())
            } else {
                Err(meta.error("expected `compile_with`, `input_type`, or `output_type`"))
            }
        })?;
    }
    let compile_with = compile_with.ok_or_else(|| {
        syn::Error::new_spanned(
            &name,
            "Modifier requires `#[modifier(compile_with = path, input_type = ..., output_type = ...)]`",
        )
    })?;
    let input_type = input_type
        .ok_or_else(|| syn::Error::new_spanned(&name, "Modifier requires a static `input_type`"))?;
    let output_type = output_type.ok_or_else(|| {
        syn::Error::new_spanned(&name, "Modifier requires a static `output_type`")
    })?;
    Ok(quote! {
        impl #name {
            pub const MODIFIER_INPUT_TYPE: ::engine::input::InputType = #input_type;
            pub const MODIFIER_OUTPUT_TYPE: ::engine::input::InputType = #output_type;
        }

        impl ::engine::input::modifier::Modifier for #name {
            fn input_type(&self) -> ::engine::input::InputType {
                Self::MODIFIER_INPUT_TYPE
            }

            fn output_type(&self) -> ::engine::input::InputType {
                Self::MODIFIER_OUTPUT_TYPE
            }

            fn create_instance(
                &self,
            ) -> ::engine::input::modifier::anyhow_crate::Result<
                Box<dyn ::engine::input::modifier::InputModifier>
            > {
                #compile_with(self)
            }
        }

        ::engine::input::modifier::inventory::submit! {
            ::engine::input::modifier::ModifierRegistration {
                type_name: <#name as ::engine::data::data_asset::DataAssetType>::TYPE_NAME,
                cast: |asset: &dyn ::engine::data::data_asset::DataAsset| {
                    asset
                        .as_any()
                        .downcast_ref::<#name>()
                        .map(|asset| asset as &dyn ::engine::input::modifier::Modifier)
                },
            }
        }
    })
}

fn expand_data_asset(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = input.ident;
    if !matches!(input.data, Data::Struct(_)) {
        return Err(syn::Error::new_spanned(
            name,
            "DataAsset can only be derived for structs",
        ));
    }
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "DataAsset cannot be derived for generic types",
        ));
    }
    let mut type_name = None;
    let mut serialize_with = None;
    let mut deserialize_with = None;
    for attribute in input
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("data_asset"))
    {
        attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("type_name") {
                let value: LitStr = meta.value()?.parse()?;
                if type_name.replace(value).is_some() {
                    return Err(meta.error("data asset type_name may only be specified once"));
                }
            } else if meta.path.is_ident("serialize_with") {
                let path: Path = meta.value()?.parse()?;
                if serialize_with.replace(path).is_some() {
                    return Err(meta.error("serialize_with may only be specified once"));
                }
            } else if meta.path.is_ident("deserialize_with") {
                let path: Path = meta.value()?.parse()?;
                if deserialize_with.replace(path).is_some() {
                    return Err(meta.error("deserialize_with may only be specified once"));
                }
            } else {
                return Err(
                    meta.error("expected `type_name`, `serialize_with`, or `deserialize_with`")
                );
            }
            Ok(())
        })?;
    }

    let type_name = type_name
        .map(|name| quote!(#name))
        .unwrap_or_else(|| quote!(stringify!(#name)));
    let factory = if let Some(deserialize_with) = deserialize_with {
        quote! {
            |data: &::engine::data::data_asset::serde_yaml::Value,
             context: &::engine::data::data_asset::DataAssetDeserializeContext| {
                let asset: #name = #deserialize_with(data, context)?;
                Ok(Box::new(asset) as Box<dyn ::engine::data::DataAsset>)
            }
        }
    } else {
        quote! {
            |data: &::engine::data::data_asset::serde_yaml::Value,
             _context: &::engine::data::data_asset::DataAssetDeserializeContext| {
                let asset: #name =
                    ::engine::data::data_asset::serde_yaml::from_value(data.clone())?;
                Ok(Box::new(asset) as Box<dyn ::engine::data::DataAsset>)
            }
        }
    };
    let serializer = if let Some(serialize_with) = serialize_with {
        quote!(#serialize_with(self))
    } else {
        quote! {
            ::engine::data::data_asset::serde_yaml::to_value(self)
                .map_err(::engine::data::data_asset::anyhow_crate::Error::from)
        }
    };
    Ok(quote! {
        impl ::engine::data::data_asset::DataAsset for #name {
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }

            fn serialize_data(
                &self,
            ) -> ::engine::data::data_asset::anyhow_crate::Result<
                ::engine::data::data_asset::serde_yaml::Value
            > {
                #serializer
            }
        }

        impl ::engine::data::data_asset::DataAssetType for #name {
            const TYPE_NAME: &'static str = #type_name;
        }

        ::engine::data::data_asset::inventory::submit! {
            ::engine::data::data_asset::DataAssetRegistration {
                type_name: #type_name,
                factory: #factory,
            }
        }
    })
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
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "Component cannot be derived for generic types",
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

    let (serialize_with, deserialize_with) = component_factory_options(&input.attrs)?;
    let has_serialize = derives_trait(&input.attrs, "Serialize");
    let has_deserialize = derives_trait(&input.attrs, "Deserialize");
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let requirement_values = requirements
        .iter()
        .map(|path| quote!(stringify!(#path)))
        .collect::<Vec<_>>();
    let factory = if let Some(deserialize_with) = deserialize_with {
        quote! {
            Some(|data: &::engine::ecs::component::serde_yaml::Value,
                  context: Option<&dyn ::std::any::Any>| {
                let component: #name #ty_generics = #deserialize_with(data, context)?;
                Ok(Box::new(component) as Box<dyn ::engine::ecs::component::Component>)
            })
        }
    } else if has_deserialize {
        quote! {
            Some(|data: &::engine::ecs::component::serde_yaml::Value,
                  _context: Option<&dyn ::std::any::Any>| {
                let component: #name #ty_generics =
                    ::engine::ecs::component::serde_yaml::from_value(data.clone())?;
                Ok(Box::new(component) as Box<dyn ::engine::ecs::component::Component>)
            })
        }
    } else {
        quote!(None)
    };
    let serializer = if let Some(serialize_with) = serialize_with {
        quote! {
            Some(|component: &dyn ::engine::ecs::component::Component| {
                let component = component
                    .as_any()
                    .downcast_ref::<#name #ty_generics>()
                    .ok_or_else(|| ::engine::ecs::component::anyhow_crate::anyhow!(
                        "component serializer received the wrong type for '{}'",
                        stringify!(#name)
                    ))?;
                #serialize_with(component)
            })
        }
    } else if has_serialize {
        quote! {
            Some(|component: &dyn ::engine::ecs::component::Component| {
                let component = component
                    .as_any()
                    .downcast_ref::<#name #ty_generics>()
                    .ok_or_else(|| ::engine::ecs::component::anyhow_crate::anyhow!(
                        "component serializer received the wrong type for '{}'",
                        stringify!(#name)
                    ))?;
                ::engine::ecs::component::serde_yaml::to_value(component)
                    .map_err(::engine::ecs::component::anyhow_crate::Error::from)
            })
        }
    } else {
        quote!(None)
    };
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
            const TYPE_NAME: &'static str = stringify!(#name);
            const REQUIRED_COMPONENTS: &'static [&'static str] = &[#(#requirement_values),*];
            const UNIQUE: bool = #unique;
        }

        ::engine::ecs::component::inventory::submit! {
            ::engine::ecs::component::ComponentRegistration {
                type_name: stringify!(#name),
                required_components: &[#(#requirement_values),*],
                unique: #unique,
                factory: #factory,
                serializer: #serializer,
                is_type: |component| component.as_any().is::<#name #ty_generics>(),
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

fn component_factory_options(
    attributes: &[Attribute],
) -> syn::Result<(Option<Path>, Option<Path>)> {
    let mut serialize_with = None;
    let mut deserialize_with = None;
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("component_factory"))
    {
        attribute.parse_nested_meta(|meta| {
            let path: Path = meta.value()?.parse()?;
            let target = if meta.path.is_ident("serialize_with") {
                &mut serialize_with
            } else if meta.path.is_ident("deserialize_with") {
                &mut deserialize_with
            } else {
                return Err(meta.error("expected `serialize_with` or `deserialize_with`"));
            };
            if target.replace(path).is_some() {
                return Err(meta.error("component serde hooks may only be specified once"));
            }
            Ok(())
        })?;
    }
    Ok((serialize_with, deserialize_with))
}

fn derives_serde(attributes: &[Attribute]) -> bool {
    derives_trait(attributes, "Serialize") || derives_trait(attributes, "Deserialize")
}

fn derives_trait(attributes: &[Attribute], trait_name: &str) -> bool {
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
                segment.ident == Ident::new(trait_name, segment.ident.span())
            })
        })
}
