use crate::cli::{Cmd, Ctx, Flag};
use crate::client::linear_failure;
use crate::fail::{Fail, R, usage};
use crate::gql;
use crate::json;
use crate::run::Run;
use crate::util::*;
use serde_json::{Map, Value, json};

/// The request type Go decoded --json-input into, as its errors name it.
const REQUEST_TYPE: &str = "struct { Query string \"json:\\\"query\\\"\"; Variables map[string]interface {} \"json:\\\"variables\\\"\"; OperationName string \"json:\\\"operationName\\\"\" }";

pub fn command() -> Cmd {
    let query = Cmd::new("query", "Send a GraphQL query (read-only) and print Linear's answer")
        .long(
            "Send a GraphQL query and print Linear's answer as JSON: data, errors, and the\n\
query's complexity (Linear allows 10,000 points per query). The request is\n\
{\"query\": \"…\", \"variables\": {…}, \"operationName\": \"…\"} from --json-input.\n\
Mutations and subscriptions are refused: changes go through the typed commands,\n\
which preview with --dry-run. Errors alongside data are exit 3, as problems.\n\
Schema and examples: https://linear.app/developers/graphql",
        )
        .example(
            "  echo '{\"query\":\"query { viewer { name } }\"}' | linearctl api query --json-input -\n  linearctl api query --json-input req.json --signed-urls 300",
        )
        .run(query)
        .flag(Flag::str("json-input", "read the request from `PATH` (- for stdin)"))
        .flag(Flag::whole("signed-urls", 0, "sign file URLs for `SECONDS` so they open without the key"));
    Cmd::group("api", "Send your own GraphQL query to Linear", vec![query])
}

/// --json-input as Go's decoder read it into {query, variables,
/// operationName}: keys match ignoring case, the last one wins.
fn request(data: &[u8]) -> R<(String, Value, String)> {
    let bad = |e: String| usage(format!("--json-input is not a JSON request object: {e}"));
    let v = json::parse(data).map_err(bad)?;
    let (mut q, mut vars, mut op) = (String::new(), Value::Null, String::new());
    let m = match v {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        other => return Err(bad(json::type_error(&other, "", REQUEST_TYPE))),
    };
    let mut first_error = None;
    for (k, x) in m {
        let field = ["query", "variables", "operationName"].into_iter().find(|f| f.eq_ignore_ascii_case(&k));
        match (field, x) {
            (None, _) | (Some(_), Value::Null) => {}
            (Some("query"), Value::String(s)) => q = s,
            (Some("operationName"), Value::String(s)) => op = s,
            (Some("variables"), Value::Object(o)) => vars = Value::Object(o),
            (Some(f), other) => {
                let ty = if f == "variables" { "map[string]interface {}" } else { "string" };
                first_error.get_or_insert_with(|| json::type_error(&other, f, ty));
            }
        }
    }
    if let Some(e) = first_error {
        return Err(bad(e));
    }
    Ok((q, vars, op))
}

/// A path element as Go's fmt.Sprint wrote it.
fn sprint(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => json::sprint_float(n.as_f64().unwrap_or(0.0)),
        other => json::marshal(other),
    }
}

fn query(run: &mut Run, ctx: &Ctx) -> R {
    let path = ctx.str("json-input");
    if path.is_empty() {
        return Err(usage("pass the request with --json-input PATH (- for stdin)"));
    }
    let data = read_input("--json-input", &path)?;
    let (q, vars, op) = request(&data)?;
    if q.trim().is_empty() {
        return Err(usage("--json-input has no query"));
    }
    let doc = gql::parse(&q).map_err(|e| usage(format!("the query does not parse: {e}")))?;
    for o in &doc.operations {
        if o.kind != "query" {
            return Err(usage(format!(
                "api query sends queries only; this is a {}. Use the typed commands to change things",
                o.kind
            )));
        }
    }
    let cl = run.client()?;
    cl.signed_urls.set(ctx.whole("signed-urls"));
    let mut body = json!({"query": q, "variables": vars});
    if !op.is_empty() {
        body["operationName"] = json!(op);
    }
    let expires = cl.signed_until();
    let a = cl.send(run, body)?;
    // Linear's answer as it came: data and errors, with each error's path
    // and extensions (code, userPresentableMessage), for a caller that
    // reads them. Failing outright (no data) also carries error and usage,
    // like every failure under --json.
    let failure = (!a.errors.is_empty() && a.data.is_null()).then(|| linear_failure(&a.errors));
    let mut out = json!({"data": a.data, "complexity": cl.complexity()});
    if !a.errors.is_empty() {
        out["errors"] = json!(a.errors);
    }
    if let Some(f) = &failure {
        out["error"] = json!(f.to_string());
        out["usage"] = json!(false);
        out["code"] = json!(f.kind());
        run.print_json_keeping(out, &["data", "errors"])?;
        return Err(Fail::Reported(f.to_string()));
    }
    if !expires.is_null() {
        out["signedUrlsExpireAt"] = expires;
    }
    if !a.errors.is_empty() {
        let problems: Vec<Value> = a
            .errors
            .iter()
            .map(|e| {
                let path: Vec<String> =
                    e["path"].as_array().map(|p| p.iter().map(sprint).collect()).unwrap_or_default();
                let where_ = if path.is_empty() { "query".to_string() } else { path.join(".") };
                json!({"input": where_, "error": e["message"].as_str().unwrap_or("")})
            })
            .collect();
        out["problems"] = json!(problems);
    }
    // Linear's data keeps its own order, as it always printed.
    run.print_json_keeping(out, &["data", "errors"])?;
    if !a.errors.is_empty() {
        return Err(Fail::Partial(format!("{} errors", a.errors.len())));
    }
    Ok(())
}
