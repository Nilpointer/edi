//! Pretty printing for JSON, XML, HTML, CSS, SQL and Markdown tables. The
//! formatters work on the source text rather than a parsed tree, so apart
//! from whitespace (and SQL keyword case) everything comes out exactly as it
//! went in: key order, number spelling, comments, entities.

mod css;
mod json;
mod markdown;
mod markup;
mod sql;

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Xml,
    Html,
    Css,
    Sql,
    Markdown,
}

/// (indent depth, line) pairs.
type Lines = Vec<(usize, String)>;

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Xml => "XML",
            Format::Html => "HTML",
            Format::Css => "CSS",
            Format::Sql => "SQL",
            Format::Markdown => "Markdown",
        }
    }

    /// The format a file is in, from its extension, then its syntax.
    pub fn of_file(path: Option<&Path>, syntax: &str) -> Option<Format> {
        let ext = path.and_then(|p| p.extension()).and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        let by_ext = match ext.as_deref().unwrap_or("") {
            "json" | "geojson" | "topojson" | "webmanifest" | "har" | "ipynb" | "jsonld" => Some(Format::Json),
            "xml" | "xsd" | "xsl" | "xslt" | "svg" | "plist" | "rss" | "atom" | "wsdl" | "kml" | "gpx" | "xaml"
            | "csproj" | "vbproj" | "fsproj" | "vcxproj" | "props" | "targets" | "nuspec" | "resx" | "xlf" | "xliff"
            | "opml" | "storyboard" | "xib" | "iml" | "tld" | "dtd" | "xul" | "sitemap" => Some(Format::Xml),
            "html" | "htm" | "xhtml" | "shtml" => Some(Format::Html),
            "css" | "scss" | "less" | "pcss" | "postcss" => Some(Format::Css),
            "sql" | "ddl" | "dml" | "pgsql" | "psql" | "mysql" => Some(Format::Sql),
            "md" | "markdown" | "mdown" | "mkd" => Some(Format::Markdown),
            _ => None,
        };
        by_ext.or(match syntax {
            "JSON" => Some(Format::Json),
            "XML" => Some(Format::Xml),
            "HTML" => Some(Format::Html),
            "CSS" => Some(Format::Css),
            "SQL" => Some(Format::Sql),
            "Markdown" | "MultiMarkdown" => Some(Format::Markdown),
            _ => None,
        })
    }
}

/// Guess the format from the text itself.
fn sniff(text: &str) -> Option<Format> {
    let t = text.trim_start();
    let head = t.get(..t.len().min(16)).unwrap_or(t).to_ascii_lowercase();
    match t.chars().next()? {
        '{' | '[' => Some(Format::Json),
        '<' if head.starts_with("<!doctype html") || head.starts_with("<html") => Some(Format::Html),
        '<' => Some(Format::Xml),
        _ => {
            let word = head.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("");
            matches!(word, "select" | "insert" | "update" | "delete" | "with" | "create" | "alter" | "drop")
                .then_some(Format::Sql)
        }
    }
}

/// Pick a format for `text`. A whole file goes by the file's format; a
/// selection goes by what it looks like, since a JSON snippet inside a
/// Markdown file should be printed as JSON.
pub fn detect(file: Option<Format>, text: &str, whole: bool) -> Option<Format> {
    let sniffed = sniff(text);
    if whole {
        return file.or(sniffed);
    }
    match (sniffed, file) {
        (Some(Format::Xml | Format::Html), Some(f @ (Format::Xml | Format::Html))) => Some(f),
        (Some(s), _) => Some(s),
        (None, f) => f,
    }
}

/// Reformat `text` as `fmt`, indenting with `unit`. Lines after the first
/// are prefixed with `base`, so a selection keeps its surrounding indentation.
pub fn format(fmt: Format, text: &str, unit: &str, base: &str) -> Result<String, String> {
    let lines = match fmt {
        Format::Json => json::format(text)?,
        Format::Xml => markup::format(text, false)?,
        Format::Html => markup::format(text, true)?,
        Format::Css => css::format(text)?,
        Format::Sql => sql::format(text),
        Format::Markdown => {
            return match markdown::format(text) {
                (_, 0) => Err("No Markdown tables to format".into()),
                (out, _) => Ok(out),
            };
        }
    };
    let mut out = String::new();
    for (i, (depth, line)) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(base);
            }
        }
        if !line.is_empty() {
            out.push_str(&unit.repeat(*depth));
        }
        out.push_str(line);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pp(fmt: Format, src: &str) -> String {
        format(fmt, src, "  ", "").unwrap()
    }

    #[test]
    fn detects_by_file_then_content() {
        let md = Format::of_file(Some(Path::new("README.MD")), "Plain Text");
        assert_eq!(md, Some(Format::Markdown));
        assert_eq!(Format::of_file(Some(Path::new("a.svg")), "Plain Text"), Some(Format::Xml));
        assert_eq!(Format::of_file(None, "JSON"), Some(Format::Json));
        assert_eq!(detect(md, "{\"a\":1}", false), Some(Format::Json));
        assert_eq!(detect(md, "{\"a\":1}", true), Some(Format::Markdown));
        assert_eq!(detect(Some(Format::Html), "<div>", false), Some(Format::Html));
        assert_eq!(detect(None, "<!DOCTYPE html><p>", true), Some(Format::Html));
        assert_eq!(detect(None, "<a/>", true), Some(Format::Xml));
        assert_eq!(detect(None, "  SELECT 1", true), Some(Format::Sql));
        assert_eq!(detect(None, "hello", true), None);
    }

    #[test]
    fn json_keeps_order_and_numbers() {
        let src = r#"{"b":1.50,"a":[1,2,{}],"s":"x,{\"y\":[]}","e":[ ]}"#;
        let want = "{\n  \"b\": 1.50,\n  \"a\": [\n    1,\n    2,\n    {}\n  ],\n  \"s\": \"x,{\\\"y\\\":[]}\",\n  \"e\": []\n}";
        assert_eq!(pp(Format::Json, src), want);
        assert!(format(Format::Json, "{\"a\":}", "  ", "").unwrap_err().starts_with("Invalid JSON"));
    }

    #[test]
    fn xml_indents_elements() {
        let src = "<?xml version=\"1.0\"?><!-- c --><root a=\"x>y\"><item>text</item><empty/><e></e>\n  <list><i>1</i></list><p>Hi <b>you</b>!</p></root>";
        let want = "<?xml version=\"1.0\"?>\n<!-- c -->\n<root a=\"x>y\">\n  <item>text</item>\n  <empty/>\n  <e></e>\n  <list>\n    <i>1</i>\n  </list>\n  <p>Hi <b>you</b>!</p>\n</root>";
        assert_eq!(pp(Format::Xml, src), want);
        assert!(format(Format::Xml, "<a><b></a>", "  ", "").unwrap_err().contains("does not match"));
        assert!(format(Format::Xml, "<a>", "  ", "").unwrap_err().contains("never closed"));
    }

    #[test]
    fn html_understands_void_raw_and_inline() {
        let src = "<!DOCTYPE html><html><head><meta charset=utf-8><style>body{margin:0}</style></head>\
                   <body><ul><li>One<li>Two <a\n href='#'>link</a></ul><pre>  keep\n   this</pre><br>\
                   <script>\n    if (a < b) {\n      go();\n    }\n</script></body></html>";
        let want = "<!DOCTYPE html>\n<html>\n  <head>\n    <meta charset=utf-8>\n    <style>\n      body {\n        margin: 0;\n      }\n    </style>\n  </head>\n  <body>\n    <ul>\n      <li>One\n      <li>Two <a href='#'>link</a>\n    </ul>\n    <pre>  keep\n   this</pre>\n    <br>\n    <script>\n      if (a < b) {\n        go();\n      }\n    </script>\n  </body>\n</html>";
        assert_eq!(pp(Format::Html, src), want);
    }

    #[test]
    fn css_one_declaration_per_line() {
        let src = "/* top */\n@import 'a.css';\na:hover,b{color:red;background:url(data:x;y)}\n@media (max-width:9px){.x{top:0}}\n.e{}\n.s{ &:hover { c : d } $v:#{$w}px }";
        let want = "/* top */\n@import 'a.css';\na:hover,b {\n  color: red;\n  background: url(data:x;y);\n}\n\n@media (max-width:9px) {\n  .x {\n    top: 0;\n  }\n}\n\n.e {}\n\n.s {\n  &:hover {\n    c: d;\n  }\n  $v: #{$w}px;\n}";
        assert_eq!(pp(Format::Css, src), want);
        assert!(format(Format::Css, "a{", "  ", "").is_err());
    }

    #[test]
    fn sql_breaks_clauses_and_lists() {
        let src = "select a, count(*) as n from t left join u on u.id = t.id and u.x=-1 where a between 1 and 2 or b in (select b from v) group by a order by n desc; insert into t (a, b) values (1, 'x''y'), (2, null);";
        let want = "SELECT\n  a,\n  count(*) AS n\nFROM t\nLEFT JOIN u ON u.id = t.id\n  AND u.x = -1\nWHERE a BETWEEN 1 AND 2\n  OR b IN (\n    SELECT b\n    FROM v\n  )\nGROUP BY a\nORDER BY n DESC;\n\nINSERT INTO t (a, b)\nVALUES\n  (1, 'x''y'),\n  (2, NULL);";
        assert_eq!(pp(Format::Sql, src), want);
        let ddl = "create table t (id int primary key, name varchar(20) not null);";
        assert_eq!(pp(Format::Sql, ddl), "CREATE TABLE t (\n  id int PRIMARY KEY,\n  name varchar(20) NOT NULL\n);");
    }

    #[test]
    fn markdown_aligns_tables_only() {
        let src = "# T\n\n|a|b|c|\n|:-|:-:|-:|\n|long cell|x|1|\n\n```\n|x|y|\n|-|-|\n```\n";
        let want = "# T\n\n| a         |  b  |   c |\n| :-------- | :-: | --: |\n| long cell |  x  |   1 |\n\n```\n|x|y|\n|-|-|\n```\n";
        assert_eq!(pp(Format::Markdown, src), want);
        assert!(format(Format::Markdown, "no tables", "  ", "").is_err());
    }

    #[test]
    fn never_panics_on_junk() {
        let junk = [
            "", " ", "<", "</", "<a", "<!--", "<![CDATA[", "<?x", "<a b='>", "}", "{", "a{b:c", "/*", "'", "\"", "$$",
            "$a$ x", "--", "1e", "((((", "))))", "|", "|-|", "a|b\n|-", "ä<ö>ü</ö>", "<script>", "<pre>x", "[1,",
            "select (", "select )", "x::int", "-- c\nselect 1", "<ul><li>a</ul></li>", "€|ß\n:-:|---",
        ];
        for fmt in [Format::Json, Format::Xml, Format::Html, Format::Css, Format::Sql, Format::Markdown] {
            for j in junk {
                let _ = format(fmt, j, "  ", " ");
            }
        }
    }

    #[test]
    fn base_indent_applies_after_first_line() {
        assert_eq!(format(Format::Json, "[1]", "  ", "    ").unwrap(), "[\n      1\n    ]");
    }
}
