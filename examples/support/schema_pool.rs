//! Losslessly intern JSON Schemas with scoped definitions for generated catalogs.
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaPool {
    pub schemas: BTreeMap<String, Value>,
    pub schema_definitions: BTreeMap<String, BTreeMap<String, String>>,
    pub definitions: BTreeMap<String, Value>,
    #[serde(skip)]
    known_schemas: BTreeMap<String, String>,
    #[serde(skip)]
    known_definitions: BTreeMap<String, String>,
}

impl SchemaPool {
    // Intern repeated schemas and definitions by canonical JSON, retaining original
    // local definition names. Different scopes can use the same name for different shapes.
    pub fn intern(&mut self, id: String, mut schema: Value) -> Result<String, serde_json::Error> {
        schema.sort_all_objects();
        let key = serde_json::to_string(&schema)?;
        if let Some(existing) = self.known_schemas.get(&key) {
            return Ok(existing.clone());
        }
        self.known_schemas.insert(key, id.clone());
        if let Some(defs) = schema.as_object_mut().and_then(|obj| obj.remove("$defs")) {
            let mut names = BTreeMap::new();
            for (name, definition) in serde_json::from_value::<BTreeMap<String, Value>>(defs)? {
                let key = serde_json::to_string(&definition)?;
                let shared = self.known_definitions.entry(key).or_insert_with(|| {
                    let unique = format!("{id}/{name}");
                    self.definitions.insert(unique.clone(), definition);
                    unique
                });
                names.insert(name, shared.clone());
            }
            self.schema_definitions.insert(id.clone(), names);
        }
        self.schemas.insert(id.clone(), schema);
        Ok(id)
    }
}
