use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context;
use pulldown_cmark::{CowStr, Event, MathMode, Options, Parser, Tag, TagEnd};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use upon::Template;

#[derive(Clone, Debug, Deserialize, Serialize)]
enum DirEntry {
    File {
        path: String,
        frontmatter: Frontmatter,
    },
    Directory {
        path: String,
        info: Option<Frontmatter>,
        contents: Vec<DirEntry>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
struct Frontmatter {
    title: String,
    description: String,
    tags: Option<BTreeSet<String>>,
    date: chrono::NaiveDate,
}

#[derive(Clone, Debug, Serialize)]
struct PageEntry {
    file: String,
    frontmatter: Option<Frontmatter>,
    date_formatted: String,
    is_dir: bool,
}

fn render_files(
    template: &Template,
    path: impl AsRef<Path>,
    md_options: Options,
    out_dir: impl AsRef<Path>,
) -> anyhow::Result<DirEntry> {
    let files = fs::read_dir(&path)?;
    let out_path = path.as_ref().display().to_string();
    let mut out_info: Option<Frontmatter> = None;
    let mut last_updated = 0;
    let mut out_contents = Vec::new();

    for file in files {
        let file = file?;
        let metadata = file.metadata()?;
        let ft = metadata.file_type();
        let path = file.path();

        let Some(file_name) = path.file_name() else {
            continue;
        };

        let file_name = file_name.to_str().context("")?.to_string();

        if ft.is_dir() {
            let mut out_dir = out_dir.as_ref().to_path_buf();
            out_dir.push(&file_name);

            fs::create_dir_all(&out_dir)?;

            out_contents.push(render_files(
                &template,
                format!("{}/{}", out_path, &file_name),
                md_options,
                out_dir,
            )?);
            continue;
        }

        if !ft.is_file() {
            continue;
        }

        if file_name == "index.toml" {
            out_info = Some(toml::from_str(&fs::read_to_string(path)?)?);
            continue;
        }

        let Some(ext) = path.extension() else {
            continue;
        };

        if ext != "md" {
            continue;
        }

        last_updated = std::cmp::max(
            last_updated,
            metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );

        let name: String = file_name.trim_end_matches(".md").into();

        eprintln!("rendering {}", path.display());

        let content = fs::read_to_string(path)?;

        let (frontmatter, content) = parse_frontmatter::<Frontmatter>(content)?;

        let parser = Parser::new_ext(&content, md_options);

        let mut heading_level = None;
        let mut quoting = false;
        let parser = parser.flat_map(|event| -> Box<dyn Iterator<Item = Event>> {
            match event {
                Event::Math(mm, s) => {
                    let opts = katex::Opts::builder()
                        .display_mode(match mm {
                            MathMode::Display => true,
                            MathMode::Inline => false,
                        })
                        .output_type(katex::opts::OutputType::Mathml)
                        .build()
                        .unwrap();

                    let mut dst = String::new();
                    match katex::render_with_opts(&s, &opts) {
                        Ok(ref ml) => dst.push_str(ml),
                        Err(err) => {
                            // gotta love these stringly typed errors
                            let err = match err {
                                e @ katex::Error::JsInitError(_) => format!("{:?}", e),
                                katex::Error::JsExecError(e) => {
                                    let mut e = e.replace("String(\"", "");

                                    if e.strip_suffix("\")").is_some() {
                                        e.pop();
                                        e.pop();
                                    }

                                    e.replace(r"\\", r"\")
                                }
                                e @ katex::Error::JsValueError(_) => format!("{:?}", e),
                                e @ _ => format!("{:?}", e),
                            };
                            eprintln!("Maths error: {:?}\n{}", err, s);
                            dst.push_str(&format!(
                                r#"<span style="color: red">Maths Error: {}</span>"#,
                                err
                            ));
                        }
                    }
                    Box::new(std::iter::once(Event::Html(dst.into())))
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    heading_level = Some(level);
                    Box::new(std::iter::empty())
                }
                Event::Start(Tag::BlockQuote) => {
                    quoting = true;
                    Box::new(std::iter::empty())
                }
                e @ Event::End(TagEnd::BlockQuote) => {
                    quoting = false;
                    Box::new(std::iter::once(e))
                }
                Event::Text(ref text) => {
                    if quoting {
                        quoting = false;
                        if let Some((left, right)) = text.split_once(':') {
                            dbg!(left, right);
                            if left.starts_with('#') {
                                let right = right.trim();
                                let subtitle = if right.starts_with('(') && right.ends_with(')') {
                                    Some(&right[1..right.len() - 1])
                                } else {
                                    None
                                };
                                let mut out = Vec::new();
                                out.push(Event::Html(
                                    format!(
                                        r#"<blockquote class="{}"><h1>{}{}</h1>"#,
                                        &left[1..],
                                        match &*left[1..].to_lowercase() {
                                            "def" => "Definition",
                                            "prop" => "Proposition",
                                            "proof" => "Proof",
                                            "ex" => "Example",
                                            "thm" => "Theorem",
                                            o => o,
                                        },
                                        if let Some(subtitle) = subtitle {
                                            format!(" <small>({})</small>", subtitle)
                                        } else {
                                            String::new()
                                        }
                                    )
                                    .into(),
                                ));
                                if subtitle.is_none() {
                                    out.push(Event::Text(right.to_string().into()));
                                }
                                Box::new(out.into_iter())
                            } else {
                                Box::new(
                                    [
                                        Event::Html(format!(r#"<blockquote>"#).into()),
                                        Event::Text(text.to_string().into()),
                                    ]
                                    .into_iter(),
                                )
                            }
                        } else {
                            Box::new(std::iter::once(Event::Text(text.to_string().into())))
                        }
                    } else if let Some(heading_level) = heading_level.take() {
                        let anchor = text
                            .clone()
                            .into_string()
                            .trim()
                            .to_lowercase()
                            .replace(" ", "-");

                        let out_tag = format!(
                            r##"<{} id="{}"><a class="header" href="#{1}">{}</a>"##,
                            heading_level, anchor, text
                        );

                        Box::new(std::iter::once(Event::Html(CowStr::from(out_tag))))
                    } else {
                        Box::new(std::iter::once(Event::Text(CowStr::from(text.to_string()))))
                    }
                }
                _ => Box::new(std::iter::once(event)),
            }
        });

        let mut content = String::new();
        pulldown_cmark::html::push_html(&mut content, parser);

        let mut out_dir = out_dir.as_ref().to_path_buf();
        out_dir.push(&name);

        fs::create_dir_all(&out_dir)?;

        out_dir.push("index.html");

        let out = template
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
        out_contents.push(DirEntry::File {
            path: format!("{}/{}", out_path, name.clone()),
            frontmatter,
        });
    }

    let (mut pages, dirs): (Vec<_>, Vec<_>) = out_contents
        .iter()
        .filter(|e| {
            !PathBuf::from(match e {
                DirEntry::File { path, .. } => path,
                DirEntry::Directory { path, .. } => path,
            })
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with('_')
        })
        .map(|e| match e {
            DirEntry::File { path, frontmatter } => {
                let path = PathBuf::from(path);
                let path = PathBuf::from_iter(path.components().skip(1));
                PageEntry {
                    file: path.display().to_string(),
                    frontmatter: Some(frontmatter.clone()),
                    date_formatted: frontmatter.date.format("%d %B %Y").to_string(),
                    is_dir: false,
                }
            }
            DirEntry::Directory { path, info, .. } => {
                let path = PathBuf::from(path);
                let path = PathBuf::from_iter(path.components().skip(1));
                PageEntry {
                    file: path.display().to_string(),
                    frontmatter: info.clone(),
                    date_formatted: if let Some(fm) = info {
                        fm.date.format("%d %B %Y").to_string()
                    } else {
                        "".into()
                    },
                    is_dir: true,
                }
            }
        })
        .partition(|e| !e.is_dir);

    pages.sort_by_key(|p| {
        (
            std::cmp::Reverse(
                p.frontmatter
                    .clone()
                    .expect("pages always have frontmatter")
                    .date,
            ),
            p.file.clone(),
        )
    });

    eprintln!("rendering Index for {}", out_path);
    let out = template
        .render(upon::value! {
            title: out_info.clone().map(|fm| fm.title),
            desc: out_info.clone().map(|fm| fm.description),
            index: true,
            pages: dirs.into_iter().chain(pages).collect::<Vec<_>>(),
            head: None,
        })
        .to_string()
        .context("Error rendering index")?;

    let mut out_dir = out_dir.as_ref().to_path_buf();
    out_dir.push("index.html");

    fs::write(out_dir, out)?;

    if let Some(ref mut out_info) = out_info {
        out_info.date = chrono::NaiveDateTime::from_timestamp_opt(last_updated as i64, 0)
            .unwrap()
            .into();
    }

    Ok(DirEntry::Directory {
        path: out_path,
        info: out_info,
        contents: out_contents,
    })
}

fn main() -> anyhow::Result<()> {
    let time = std::time::Instant::now();

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
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH;

    render_files(&layout_template, "md", md_options, out_dir)?;

    eprintln!("Completed rendering in {}ms", time.elapsed().as_millis());
    Ok(())
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
