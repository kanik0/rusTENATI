use std::io::Write;

use anyhow::Result;
use clap::{Args, Subcommand};
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use crate::download::state::StateDb;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum GraphAction {
    /// Build the knowledge graph from OCR tags
    Build(GraphBuildArgs),

    /// Search for a person in the graph
    Query(GraphQueryArgs),

    /// Show ancestors of a person
    Ancestors(GraphAncestorsArgs),

    /// Export graph in DOT format for visualization
    Export(GraphExportArgs),

    /// Show graph statistics
    Stats,
}

#[derive(Debug, Args)]
pub struct GraphBuildArgs;

#[derive(Debug, Args)]
pub struct GraphQueryArgs {
    /// Search query (surname or name)
    pub query: String,
}

#[derive(Debug, Args)]
pub struct GraphAncestorsArgs {
    /// Graph node ID
    pub node_id: i64,

    /// Maximum depth to traverse (default: 10)
    #[arg(short, long, default_value = "10")]
    pub depth: usize,
}

#[derive(Debug, Args)]
pub struct GraphExportArgs {
    /// Output format: dot, json
    #[arg(short, long, default_value = "dot")]
    pub format: String,

    /// Output file (default: stdout)
    #[arg(short, long)]
    pub output: Option<String>,
}

pub fn run(action: &GraphAction, json: bool) -> Result<()> {
    let state_db = StateDb::open(&output::db_path())?;

    match action {
        GraphAction::Build(_) => build_graph(&state_db),
        GraphAction::Query(args) => query_graph(&state_db, &args.query, json),
        GraphAction::Ancestors(args) => show_ancestors(&state_db, args.node_id, args.depth, json),
        GraphAction::Export(args) => export_graph(&state_db, args),
        GraphAction::Stats => show_stats(&state_db, json),
    }
}

fn build_graph(db: &StateDb) -> Result<()> {
    eprintln!("Building knowledge graph from OCR tags...");

    // Get all tags grouped by download_id
    let tags = db.search_tags(None, None)?;

    if tags.is_empty() {
        eprintln!("No tags found. Run OCR with --extract-tags first.");
        return Ok(());
    }

    // Group tags by (manifest_id, canvas_id) to process each document
    let mut docs: std::collections::HashMap<(String, String), Vec<&crate::download::state::TagRecord>> =
        std::collections::HashMap::new();

    for tag in &tags {
        docs.entry((tag.manifest_id.clone(), tag.canvas_id.clone()))
            .or_default()
            .push(tag);
    }

    let mut nodes_created = 0usize;
    let mut edges_created = 0usize;

    for ((manifest_id, _canvas_id), doc_tags) in &docs {
        // Extract people and roles from this document
        let mut surnames: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut roles: Vec<(String, String)> = Vec::new(); // (role, name)
        let mut event_type: Option<String> = None;

        for tag in doc_tags {
            match tag.tag_type.as_str() {
                "surname" => surnames.push(tag.value.clone()),
                "name" => names.push(tag.value.clone()),
                "role" => {
                    // Format: "ruolo: nome"
                    if let Some((role, name)) = tag.value.split_once(": ") {
                        roles.push((role.to_string(), name.to_string()));
                    }
                }
                "event_type" => event_type = Some(tag.value.clone()),
                _ => {}
            }
        }

        // Create nodes for each unique person mentioned
        let mut person_nodes: Vec<(i64, String, String)> = Vec::new(); // (node_id, role, full_name)

        for (role, name) in &roles {
            // Try to match the name with known surnames
            let parts: Vec<&str> = name.split_whitespace().collect();
            let (given, surname) = if parts.len() >= 2 {
                // Heuristic: last word is surname for Italian names
                let surname_candidate = parts.last().unwrap();
                let given_candidate = parts[..parts.len() - 1].join(" ");
                // Check if any known surname matches
                if surnames.iter().any(|s| s.eq_ignore_ascii_case(surname_candidate)) {
                    (given_candidate, surname_candidate.to_string())
                } else {
                    (name.clone(), String::new())
                }
            } else {
                (name.clone(), String::new())
            };

            let full_name = if surname.is_empty() {
                given.clone()
            } else {
                format!("{} {}", given, surname)
            };

            let sn = if surname.is_empty() { None } else { Some(surname.as_str()) };
            let gn = if given.is_empty() { None } else { Some(given.as_str()) };

            let node_id = db.upsert_graph_node(&full_name, sn, gn, None)?;
            if node_id > 0 {
                nodes_created += 1;
            }
            person_nodes.push((node_id, role.clone(), full_name));
        }

        // Create edges based on event type and roles
        let event = event_type.as_deref().unwrap_or("unknown");

        // For birth records: padre/madre → child
        if event.contains("nascita") || event.contains("birth") {
            let child_node = person_nodes.iter()
                .find(|(_, role, _)| role.is_empty() || role == "neonato" || role == "nato");
            let father_node = person_nodes.iter()
                .find(|(_, role, _)| role == "padre" || role == "father");
            let mother_node = person_nodes.iter()
                .find(|(_, role, _)| role == "madre" || role == "mother");

            if let Some((child_id, _, _)) = child_node {
                if let Some((father_id, _, _)) = father_node {
                    db.insert_graph_edge(*father_id, *child_id, "parent_of", None, None, Some(manifest_id))?;
                    edges_created += 1;
                }
                if let Some((mother_id, _, _)) = mother_node {
                    db.insert_graph_edge(*mother_id, *child_id, "parent_of", None, None, Some(manifest_id))?;
                    edges_created += 1;
                }
            }
        }

        // For marriage records: sposo ↔ sposa
        if event.contains("matrimonio") || event.contains("marriage") {
            let groom = person_nodes.iter()
                .find(|(_, role, _)| role == "sposo" || role == "groom");
            let bride = person_nodes.iter()
                .find(|(_, role, _)| role == "sposa" || role == "bride");

            if let (Some((groom_id, _, _)), Some((bride_id, _, _))) = (groom, bride) {
                db.insert_graph_edge(*groom_id, *bride_id, "spouse_of", None, None, Some(manifest_id))?;
                edges_created += 1;
            }

            // Witnesses
            for (node_id, role, _) in &person_nodes {
                if role == "testimone" || role == "witness" {
                    if let Some((groom_id, _, _)) = groom {
                        db.insert_graph_edge(*node_id, *groom_id, "witness_for", None, None, Some(manifest_id))?;
                        edges_created += 1;
                    }
                }
            }
        }

        // For death records: connect to spouse if mentioned
        if event.contains("morte") || event.contains("death") {
            let deceased = person_nodes.iter()
                .find(|(_, role, _)| role == "defunto" || role == "deceased" || role.is_empty());
            let spouse = person_nodes.iter()
                .find(|(_, role, _)| role == "coniuge" || role == "spouse" || role == "vedova" || role == "vedovo");

            if let (Some((deceased_id, _, _)), Some((spouse_id, _, _))) = (deceased, spouse) {
                db.insert_graph_edge(*deceased_id, *spouse_id, "spouse_of", None, None, Some(manifest_id))?;
                edges_created += 1;
            }
        }
    }

    eprintln!(
        "Knowledge graph built: {} nodes, {} edges from {} documents",
        nodes_created,
        edges_created,
        docs.len()
    );

    Ok(())
}

fn query_graph(db: &StateDb, query: &str, json: bool) -> Result<()> {
    let nodes = db.search_graph_nodes(query)?;

    if nodes.is_empty() {
        eprintln!("No persons found matching '{}'", query);
        return Ok(());
    }

    if json {
        let mut results = Vec::new();
        for node in &nodes {
            let relationships = db.get_relationships(node.id)?;
            results.push(serde_json::json!({
                "node": node,
                "relationships": relationships,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    for node in &nodes {
        println!(
            "\n[{}] {} {} (ID: {})",
            if node.person_id.is_some() { "linked" } else { "graph" },
            node.given_name.as_deref().unwrap_or(""),
            node.surname.as_deref().unwrap_or(""),
            node.id,
        );

        let relationships = db.get_relationships(node.id)?;
        if relationships.is_empty() {
            println!("  No relationships found");
        } else {
            let mut table = Table::new();
            table.load_preset(UTF8_FULL_CONDENSED);
            table.set_header(vec!["Relationship", "Person", "Direction"]);

            for rel in &relationships {
                let name = format!(
                    "{} {}",
                    rel.related_given_name.as_deref().unwrap_or(""),
                    rel.related_surname.as_deref().unwrap_or("")
                );
                table.add_row(vec![
                    &rel.relationship,
                    &name,
                    &rel.direction,
                ]);
            }
            println!("{table}");
        }
    }

    Ok(())
}

fn show_ancestors(db: &StateDb, node_id: i64, max_depth: usize, json: bool) -> Result<()> {
    let ancestors = db.get_ancestors(node_id, max_depth)?;

    if ancestors.is_empty() {
        eprintln!("No ancestors found for node {}", node_id);
        return Ok(());
    }

    if json {
        let output: Vec<serde_json::Value> = ancestors
            .iter()
            .map(|(node, depth)| {
                serde_json::json!({
                    "node": node,
                    "generation": depth,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("Ancestors of node {}:", node_id);
    for (node, depth) in &ancestors {
        let indent = "  ".repeat(*depth);
        let gen_label = match depth {
            1 => "parent",
            2 => "grandparent",
            3 => "great-grandparent",
            n => &format!("gen-{n}"),
        };
        println!(
            "{}{} {} ({}) [{}]",
            indent,
            node.given_name.as_deref().unwrap_or("?"),
            node.surname.as_deref().unwrap_or("?"),
            gen_label,
            node.id,
        );
    }

    Ok(())
}

fn export_graph(db: &StateDb, args: &GraphExportArgs) -> Result<()> {
    let mut writer: Box<dyn Write> = if let Some(ref path) = args.output {
        Box::new(std::fs::File::create(path)?)
    } else {
        Box::new(std::io::stdout())
    };

    match args.format.as_str() {
        "dot" => {
            writeln!(writer, "digraph family_tree {{")?;
            writeln!(writer, "  rankdir=BT;")?;
            writeln!(writer, "  node [shape=box, style=filled, fillcolor=lightyellow];")?;
            writeln!(writer, "")?;

            // Export all nodes
            let nodes = db.search_graph_nodes("")?;
            for node in &nodes {
                let label = format!(
                    "{} {}",
                    node.given_name.as_deref().unwrap_or(""),
                    node.surname.as_deref().unwrap_or("")
                );
                writeln!(writer, "  n{} [label=\"{}\"];", node.id, label.trim())?;
            }
            writeln!(writer, "")?;

            // Export all edges
            for node in &nodes {
                let rels = db.get_relationships(node.id)?;
                for rel in &rels {
                    if rel.direction == "outgoing" {
                        let style = match rel.relationship.as_str() {
                            "parent_of" => "color=blue",
                            "spouse_of" => "color=red, style=dashed, dir=both",
                            "witness_for" => "color=gray, style=dotted",
                            _ => "",
                        };
                        writeln!(
                            writer,
                            "  n{} -> n{} [label=\"{}\", {}];",
                            node.id, rel.related_node_id, rel.relationship, style
                        )?;
                    }
                }
            }

            writeln!(writer, "}}")?;
        }
        "json" => {
            let nodes = db.search_graph_nodes("")?;
            let mut edges = Vec::new();
            for node in &nodes {
                let rels = db.get_relationships(node.id)?;
                for rel in &rels {
                    if rel.direction == "outgoing" {
                        edges.push(serde_json::json!({
                            "source": node.id,
                            "target": rel.related_node_id,
                            "relationship": rel.relationship,
                        }));
                    }
                }
            }
            let output = serde_json::json!({
                "nodes": nodes,
                "edges": edges,
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&output)?)?;
        }
        f => anyhow::bail!("Unsupported format: {f}. Use: dot, json"),
    }

    Ok(())
}

fn show_stats(db: &StateDb, json: bool) -> Result<()> {
    let stats = db.get_graph_stats()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Knowledge Graph Statistics:");
        println!("  Nodes (persons): {}", stats.nodes);
        println!("  Edges (relationships): {}", stats.edges);
    }

    Ok(())
}
