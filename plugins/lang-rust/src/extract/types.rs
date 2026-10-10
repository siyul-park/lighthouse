//! The declarations of types: the type itself, the fields of a struct and the
//! variants of an enum, with the type of each field.

use lighthouse_protocol::{Node, Symbol, SymbolKind};
use quote::ToTokens;
use syn::Attribute;

use super::{Extractor, Owner, Shape, cap, declared};
use crate::{
    names::{ModId, symbol_id},
    signature::{field_type, optional_field, serde_default},
    tree::vis,
    util::{doc_of, extent_of, span_of},
};

impl Extractor<'_> {
    pub(super) fn type_item(
        &mut self,
        m: ModId,
        ident: &syn::Ident,
        v: &syn::Visibility,
        attrs: &[Attribute],
        item: &impl ToTokens,
        shape: Shape,
    ) {
        let name = ident.to_string();
        let module = self.idx.module_path(m).to_owned();
        let id = symbol_id(&module, &[&name], SymbolKind::Type);
        let visibility = self.idx.visibility(vis(v), m, &id);
        self.declare(
            Symbol {
                id: id.clone(),
                kind: SymbolKind::Type,
                visibility,
                owner: None,
                span: span_of(self.src, item),
                extent: Some(extent_of(self.src, item)),
                doc: doc_of(attrs),
                name: name.clone(),
                ..self.blank()
            },
            Node::Module(module.clone()),
        );
        let owner = Owner {
            m,
            id,
            name,
            module,
            visibility,
        };
        match shape {
            Shape::Struct(fields) => self.fields(&owner, fields, serde_default(attrs)),
            Shape::Enum(e) => self.variants(&owner, e),
            Shape::Plain => {}
        }
    }

    fn fields(&mut self, owner: &Owner, fields: &syn::Fields, container_default: bool) {
        let syn::Fields::Named(named) = fields else {
            return;
        };
        for field in &named.named {
            let Some(ident) = &field.ident else {
                continue;
            };
            let name = ident.to_string();
            let id = symbol_id(&owner.module, &[&owner.name, &name], SymbolKind::Field);
            let visibility = cap(declared(vis(&field.vis)), owner.visibility);
            let optional = optional_field(field, container_default);
            self.declare(
                Symbol {
                    id,
                    kind: SymbolKind::Field,
                    visibility,
                    owner: Some(owner.id.clone()),
                    span: span_of(self.src, field),
                    extent: Some(extent_of(self.src, field)),
                    doc: doc_of(&field.attrs),
                    name,
                    ..self.blank()
                },
                Node::Symbol(owner.id.clone()),
            );
            if let Some(last) = self.frag.symbols.last_mut() {
                last.optional = optional;
                last.type_ref = Some(field_type(self.idx, owner.m, &field.ty));
            }
        }
    }

    fn variants(&mut self, owner: &Owner, e: &syn::ItemEnum) {
        for variant in &e.variants {
            let name = variant.ident.to_string();
            let id = symbol_id(&owner.module, &[&owner.name, &name], SymbolKind::Variant);
            self.declare(
                Symbol {
                    id,
                    kind: SymbolKind::Variant,
                    visibility: owner.visibility,
                    owner: Some(owner.id.clone()),
                    span: span_of(self.src, variant),
                    extent: Some(extent_of(self.src, variant)),
                    doc: doc_of(&variant.attrs),
                    name,
                    ..self.blank()
                },
                Node::Symbol(owner.id.clone()),
            );
        }
    }
}
