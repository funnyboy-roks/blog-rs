# blog-rs

This is a _super_ simple program which can translate a folder of
markdown files into statically generated website.  Note that is
effectively a highly opionated framework because I built it specifically
to fit my needs.

If anyone desires to use this (though unless your building something
small, simple, and personal, I'd recommend against it), let me know if
you run into any issues!

## Usage

The program is quite simple, it reads `.md` files from the `md/`
directory and uses the templates in `template/` to build the pages.

It will copy everything from `static/` into `build/` before any
processing happens (rather, it copies the entire directory because I'm
very lazy).

When processing, it reads the markdown file, handles the maths, then
parses the markdown and writes it into the `build/` directory under
`build/<md-file-name>/index.html` so that one can go to the name in
their browser, i.e. `https://blog.funnyboyroks.com/about`.

The templates that the program checks for are listed below:

- `head.hbs` - intended to be put into the `<head>` tag upon render
- `index.hbs` - intended to be used for the root of the site
- `layout.hbs` - the only one that gets directly rendered by the program
- `page.hbs` - intended to be used for each page on the site

Note the use of "intended". Since the templates are only loaded into the
engine and not directly rendered, the `layout.hbs` _must_ include the
others for them to be rendered.
