use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};
use regex::{Regex, Replacer};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Frontmatter {
    title: String,
    description: String,
    tags: Option<BTreeSet<String>>,
    date: chrono::NaiveDate,
}

fn main() -> anyhow::Result<()> {
    let time = std::time::Instant::now();

    let files: Vec<_> = fs::read_dir("md")
        .context("md dir not found")?
        .filter_map(|d| d.ok())
        .collect();

    let mut engine = upon::Engine::new();

    let out_dir = PathBuf::from("build");

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)?;
    }

    let static_path = Path::new("static");
    if static_path.exists() && static_path.is_dir() {
        // I'm to lazy to do this properly...
        let exit = Command::new("cp")
            .arg("-r")
            .arg("static")
            .arg("build")
            .spawn()?
            .wait()?;

        if let Some(code) = exit.code() {
            if code != 0 {
                anyhow::bail!("Failed to copy static files, exit code: {}", code)
            }
        }
    } else {
        fs::create_dir(&out_dir).context("unable to create build dir")?;
    }

    engine
        .add_template(
            "head",
            fs::read_to_string("template/head.hbs").context("template/head.hbs")?,
        )
        .context("template/head.hbs")?;

    engine
        .add_template(
            "index",
            fs::read_to_string("template/index.hbs").context("template/index.hbs")?,
        )
        .context("template/index.hbs")?;

    engine
        .add_template(
            "page",
            fs::read_to_string("template/page.hbs").context("template/page.hbs")?,
        )
        .context("template/page.hbs")?;

    let layout_content =
        &fs::read_to_string("template/layout.hbs").context("template/layout.hbs")?;
    let layout_template = engine
        .compile(layout_content)
        .context("template/layout.hbs")?;

    let md_options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    let mut pages = HashMap::with_capacity(files.len());

    for file in files {
        let metadata = file.metadata()?;
        let ft = metadata.file_type();
        let path = file.path();

        let Some(file_name) = path.file_name() else {
            continue;
        };

        let file_name = file_name.to_str().context("")?.to_string();

        if !ft.is_file() {
            continue;
        }

        let Some(ext) = path.extension() else {
            continue;
        };

        if ext != "md" {
            continue;
        }

        let name: String = file_name.trim_end_matches(".md").into();

        eprintln!("rendering {}", name);

        // We know that `file` is now a md file

        let content = fs::read_to_string(path)?;

        let (frontmatter, content) = parse_frontmatter::<Frontmatter>(content)?;

        let content = preprocess_content(&content);

        let parser = Parser::new_ext(&content, md_options);

        let mut heading_level = None;
        let parser = parser.filter_map(|event| match event {
            Event::Start(Tag::Heading(level, ..)) => {
                heading_level = Some(level);
                None
            }
            Event::Text(text) => {
                if let Some(heading_level) = heading_level.take() {
                    let anchor = text
                        .clone()
                        .into_string()
                        .trim()
                        .to_lowercase()
                        .replace(" ", "-");
                    let tmp = Event::Html(CowStr::from(format!(
                        r##"<{heading_level} id="{anchor}"><a class="header" href="#{anchor}">{text}</a>"##,
                    )))
                    .into();
                    return tmp;
                } else {
                    Some(Event::Text(text))
                }
            }
            _ => Some(event),
        });

        let mut content = String::new();
        pulldown_cmark::html::push_html(&mut content, parser);

        let mut out_dir = out_dir.clone();
        out_dir.push(&name);

        fs::create_dir_all(&out_dir)?;

        out_dir.push("index.html");

        let out = layout_template
            .render(upon::value! {
                frontmatter: &frontmatter,
                index: false,
                rendered_body: &content,
                head: None,
            })
            .to_string()
            .with_context(|| format!("Error rendering {}", name))?;

        let out = minify_html::minify(
            out.as_bytes(),
            &minify_html::Cfg {
                do_not_minify_doctype: true,
                ensure_spec_compliant_unquoted_attribute_values: true,
                keep_closing_tags: true,
                keep_html_and_head_opening_tags: true,
                keep_spaces_between_attributes: false,
                keep_comments: false,
                minify_css: true,
                minify_css_level_1: true,
                minify_css_level_2: true,
                minify_css_level_3: true,
                minify_js: true,
                remove_bangs: false,
                remove_processing_instructions: false,
            },
        );

        fs::write(out_dir, out)?;
        pages.insert(name.clone(), frontmatter);
    }

    #[derive(Clone, Debug, Serialize)]
    struct PageEntry {
        file: String,
        frontmatter: Frontmatter,
        date_formatted: String,
    }

    let mut pages: Vec<_> = pages
        .into_iter()
        .filter(|(n, _)| !n.starts_with('_'))
        .map(|(n, fm)| {
            let date_formatted = fm.date.format("%d %B %Y").to_string();
            PageEntry {
                file: n.trim_end_matches(".md").into(),
                frontmatter: fm,
                date_formatted,
            }
        })
        .collect();

    pages.sort_by_key(|p| (std::cmp::Reverse(p.frontmatter.date), p.file.clone()));

    eprintln!("rendering Index");
    let out = layout_template
        .render(upon::value! {
            index: true,
            pages: pages,
            head: None,
        })
        .to_string()
        .context("Error rendering index")?;

    let mut out_dir = out_dir.clone();
    out_dir.push("index.html");

    fs::write(out_dir, out)?;

    eprintln!("Completed rendering in {}ms", time.elapsed().as_millis());
    Ok(())
}

enum MathReplacer {
    Block,
    Inline,
}

impl Replacer for MathReplacer {
    fn replace_append(&mut self, caps: &regex::Captures<'_>, dst: &mut String) {
        let opts = katex::Opts::builder()
            .display_mode(match self {
                Self::Block => true,
                Self::Inline => false,
            })
            .output_type(katex::opts::OutputType::Mathml)
            .build()
            .unwrap();

        match katex::render_with_opts(&caps[1], &opts) {
            Ok(ref ml) => {
                dst.push_str(
                    // Note: pulldown-cmark switches these back to the correct symbols
                    &ml.replace("_", r"\_")
                        .replace("*", r"\*")
                        .replace("~", r"\~"),
                );
            }
            Err(err) => {
                dbg!(&caps[1]);
                eprintln!("Maths error: {:?}", err);
                dst.push_str(&format!(
                    r#"<span style="red">Maths Error: {:?}</span>"#,
                    err
                ));
            }
        }
    }
}

lazy_static::lazy_static! {
    static ref MATH_BLOCK: Regex = Regex::new(r"(?s)\s\$\$(.+?)\$\$\s").unwrap();
    static ref MATH_INLINE: Regex = Regex::new(r"(?s)[^\\$]\$(.+?)\$").unwrap();
}

fn preprocess_content(content: &str) -> String {
    let out = MATH_BLOCK.replace_all(content, MathReplacer::Block);
    let out = MATH_INLINE.replace_all(&out, MathReplacer::Inline);
    let out = out.replace(r"\$", "$");
    out.into()
}

fn parse_frontmatter<T>(content: String) -> anyhow::Result<(T, String)>
where
    T: DeserializeOwned,
{
    let mut lines = content.lines();

    if let Some(line) = lines.next() {
        if line != "---" {
            anyhow::bail!("Expected '---' on first line, found {}", line);
        }
    }

    let a: Vec<_> = lines.collect();
    let a = a.join("\n");

    let (fm, content) = a.split_once("---").context("expected second '---'")?;

    Ok((toml::from_str(fm)?, content.into()))
}
