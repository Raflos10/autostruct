use crate::database::InfoProvider;
use crate::rust;
use crate::{database, rust::Type};
use anyhow::Error;
use cruet::Inflector;
use std::collections::HashSet;

use super::runner::Framework;

/**
Contains fields that indicate formatting options that should be applied to the generated code

# Fields
- `singular`: specifies with the generated Rust structs name should be the singular form the provided tables
- `framework`: specifies the framework to be used for generating the code
*/
#[derive(Debug)]
pub struct Options {
    pub singular: bool,
    pub framework: Framework,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            singular: false,
            framework: Framework::None,
        }
    }
}

pub struct Generator {
    options: Options,
    provider: Box<dyn InfoProvider>,
}

impl Generator {
    pub fn new(options: Options, provider: Box<dyn InfoProvider>) -> Self {
        Self { options, provider }
    }

    pub async fn generate_code(&self) -> Result<Vec<Snippet>, Error> {
        let schema = self.provider.get_schema().await?;
        let mut snippets: Vec<Snippet> = vec![];
        snippets.append(&mut self.code_from_enums(&schema.enumerations));
        snippets.append(&mut self.code_from_composites(&schema.composite_types));
        snippets.append(&mut self.code_from_tables(&schema.tables));

        // Finalize all snippets
        for snippet in &mut snippets {
            snippet.finalize();
        }

        Ok(snippets)
    }

    fn code_from_enums(&self, enums: &[database::Enum]) -> Vec<Snippet> {
        enums
            .iter()
            .map(|e| {
                let name = e.name.to_pascal_case();
                let mut snippet = Snippet::new(name.clone());

                let macros = match self.options.framework {
                    Framework::None => "#[derive(Debug, Clone, PartialEq, Eq)]\n",
                    Framework::Sqlx => "#[derive(Debug, Clone, PartialEq, Eq, sqlx::Type)]\n",
                };

                snippet.code.push_str(macros);
                snippet.code.push_str(&format!("pub enum {} {{\n", name));

                for value in &e.values {
                    if self.options.framework == Framework::Sqlx {
                        let rename_macro = format!("    #[sqlx(rename = \"{}\")]\n", value.name);
                        snippet.code.push_str(&rename_macro);    
                    }
                    let field_name = value.name.to_pascal_case();
                    let enum_field = format!("    {field_name},\n");
                    snippet.code.push_str(&enum_field);
                }

                snippet.code.push('}');
                snippet
            })
            .collect()
    }

    fn code_from_composites(&self, composites: &[database::CompositeType]) -> Vec<Snippet> {
        composites
            .iter()
            .map(|composite| {
                let table_name = self.format_name(&composite.name);
                let mut snippet = Snippet::new(table_name.clone());
                let macros = match self.options.framework {
                    Framework::None => "#[derive(Debug, Clone)]\n",
                    Framework::Sqlx => "#[derive(Debug, Clone, sqlx::Type)]\n",
                };
                snippet.code.push_str(macros);
                snippet
                    .code
                    .push_str(&format!("pub struct {} {{\n", table_name.to_pascal_case()));

                for attr in &composite.attributes {
                    let rust_type = self.provider.type_name_from(&attr.data_type);
                    self.add_type_imports(&mut snippet, &rust_type);
                    let field_name = attr.name.clone();
                    self.add_framework_attribute(&rust_type, &mut snippet);
                    let struct_field = format!("    pub {field_name}: {rust_type},\n");
                    snippet.code.push_str(&struct_field);
                }

                snippet.code.push('}');
                snippet
            })
            .collect()
    }

    fn code_from_tables(&self, tables: &[database::Table]) -> Vec<Snippet> {
        tables
            .iter()
            .map(|table| {
                let table_name = self.format_name(&table.name);
                let mut snippet = Snippet::new(table_name.clone());
                self.add_framework_macros(&mut snippet);

                snippet
                    .code
                    .push_str(&format!("pub struct {} {{\n", table_name.to_pascal_case()));

                for column in &table.columns {
                    let mut rust_type = self.provider.type_name_from(&column.udt_name);
                    if column.is_nullable {
                        rust_type = Type::Option(Box::new(rust_type));
                    }

                    self.add_type_imports(&mut snippet, &rust_type);
                    let field_name = column.name.clone();
                    self.add_framework_attribute(&rust_type, &mut snippet);

                    let struct_field = format!("    pub {field_name}: {rust_type},\n");
                    snippet.code.push_str(&struct_field);
                }

                snippet.code.push('}');
                snippet
            })
            .collect()
    }

    fn add_type_imports(&self, snippet: &mut Snippet, rust_type: &Type) {
        match rust_type {
            Type::Uuid(_) => snippet.add_import("uuid::Uuid"),
            Type::Date(_) => snippet.add_import("chrono::NaiveDate"),
            Type::Time(_) => snippet.add_import("chrono::NaiveTime"),
            Type::Timestamp(_) => snippet.add_import("chrono::NaiveDateTime"),
            Type::TimestampWithTz(_) => {
                snippet.add_import("chrono::{DateTime, Utc}");
            }
            Type::Interval(_) => {
                snippet.add_import("sqlx::postgres::types::PgInterval");
            }
            Type::Decimal(_) => snippet.add_import("rust_decimal::Decimal"),
            Type::IpNetwork(_) => snippet.add_import("ipnetwork::IpNetwork"),
            Type::Json(_) => snippet.add_import("serde_json::Value"),
            Type::Tree(_) => snippet.add_import("postgres_types::LTree"),
            Type::Query(_) => snippet.add_import("postgres_types::TSQuery"),
            Type::Option(inner) => self.add_type_imports(snippet, inner),
            Type::Vector(inner) => self.add_type_imports(snippet, inner),
            Type::Range(inner) => {
                snippet.add_import("sqlx::postgres::types::PgRange");
                self.add_type_imports(snippet, inner);
            }
            Type::Money(_) => snippet.add_import("sqlx::postgres::types::PgMoney"),
            Type::Custom(name) => {
                if name.starts_with("postgis::") {
                    snippet.add_import("postgis");
                } else if name == "Oid" {
                    snippet.add_import("sqlx::postgres::types::Oid");
                } else if !name.contains("::") {
                    snippet.add_dependency(name);
                }
            }
            _ => {}
        }
    }

    fn add_framework_macros(&self, snippet: &mut Snippet) {
        // Add framework-specific derives and imports
        match self.options.framework {
            Framework::None => {
                snippet.code.push_str("#[derive(Debug, Clone)]\n");
            }
            Framework::Sqlx => {
                snippet
                    .code
                    .push_str("#[derive(Debug, Clone, sqlx::FromRow)]\n");
            }
        }
    }

    fn add_framework_attribute(&self, rust_type: &rust::Type, snippet: &mut Snippet) {
        if let Framework::Sqlx = self.options.framework {
            if let Type::Option(_) = rust_type {
                snippet.code.push_str("    #[sqlx(default)]\n");
            }
        }
    }

    fn format_name(&self, name: &str) -> String {
        if self.options.singular {
            name.to_singular()
        } else {
            name.to_string()
        }
    }
}

pub struct Snippet {
    pub id: String,
    pub imports: HashSet<String>,
    pub code: String,
    pub dependencies: HashSet<String>, // Track other structs this one depends on
}

impl Snippet {
    fn new(id: String) -> Self {
        Self {
            id,
            imports: HashSet::new(),
            code: String::new(),
            dependencies: HashSet::new(),
        }
    }

    fn add_import(&mut self, import: &str) {
        self.imports.insert(import.to_string());
    }

    fn add_dependency(&mut self, dependency: &str) {
        self.dependencies.insert(dependency.to_string());
    }

    fn finalize(&mut self) {
        // Add imports at the top of the code
        let mut final_code = String::new();

        // Add imports
        for import in &self.imports {
            final_code.push_str(&format!("use {};\n", import));
        }

        // Add dependencies as relative imports
        for dep in &self.dependencies {
            final_code.push_str(&format!("use super::{};\n", dep.to_pascal_case()));
        }

        if !self.imports.is_empty() || !self.dependencies.is_empty() {
            final_code.push('\n');
        }

        final_code.push_str(&self.code);
        self.code = final_code;
    }
}
