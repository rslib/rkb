-- The rkb site template for rs-web. `rkb site build` runs rs-web in a folder that holds this file,
-- `templates/`, `static/`, `site.json`, `lessons/` and, with SITE_PASSWORD set, `protected/`.
-- Change it freely after `rkb site init`. Keep lesson pages at the `url` from site.json: a public lesson
-- at its knowledge-base path (`/general/git/<file>/`), a protected one at `/protected/<id>/`. The build
-- checks that every page under `/lessons/` and `/protected/` belongs to a published lesson, and that no
-- page sits at a protected lesson's path.
local rs = require("rs-web")

local site = rs.data.load_json("site.json")
local settings = site.site or {}
local title = settings.title or "Lessons learned"
local description = (settings.description ~= nil and settings.description ~= "") and settings.description
  or "Notes on problems solved and decisions made."
-- Without `[site] base_url`, pages carry no absolute URLs and there is no sitemap.
local base_url = (settings.base_url or ""):gsub("/+$", "")
-- The link-preview image under static/: `[site] image`, staged by rkb, or the built-in one.
local og_image = (settings.image ~= nil and settings.image ~= "") and settings.image or "og.png"
-- Stylesheets and scripts by content hash (`site_css` -> `/static/v/<hash>/site.css`), from rkb.
local assets = site.assets or {}
local protected = site.protected or {}

local by_id = {}
for _, l in ipairs(site.lessons) do
  by_id[l.id] = l
end
for _, l in ipairs(protected) do
  by_id[l.id] = l
end

local TYPES = {
  pitfall = { name = "Pitfall", note = "The symptom, what causes it, and the fix that was run and seen to work." },
  recipe = { name = "Recipe", note = "When to use it, the steps, and what showed that they work." },
  fact = { name = "Fact", note = "A statement and the evidence for it." },
  decision = { name = "Decision", note = "The context, the choice, why, and the options that were turned down." },
  preference = { name = "Preference", note = "A rule, why it holds, and how to apply it." },
}
local TYPE_ORDER = { "pitfall", "recipe", "fact", "decision", "preference" }
local HOW = { ran = "ran it", read = "read it", told = "told", checked = "checked" }
local MONTHS = { "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec" }
local SCOPES = { general = "General", projects = "Projects", systems = "Systems" }

local ICONS = {
  pitfall = '<path d="M12 3 2 20h20z"/><path d="M12 10v4"/><path d="M12 17h.01"/>',
  recipe = '<path d="M9 6h11"/><path d="M9 12h11"/><path d="M9 18h11"/><path d="M4 6h.01"/><path d="M4 12h.01"/><path d="M4 18h.01"/>',
  fact = '<circle cx="12" cy="12" r="9"/><path d="M12 11v5"/><path d="M12 8h.01"/>',
  decision = '<path d="M6 3v18"/><path d="M6 12h9l3-4-3-4H6"/>',
  preference = '<path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z"/>',
  lock = '<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
}

local function svg(name, size)
  return '<svg width="' .. size .. '" height="' .. size .. '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" '
    .. 'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' .. ICONS[name] .. "</svg>"
end

local function slug(s)
  return (s:lower():gsub("[^%w]+", "-"):gsub("^-+", ""):gsub("-+$", ""))
end

local function escape_pattern(s)
  return (s:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"))
end

local function esc(s)
  return (tostring(s):gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;"))
end

local function date(iso)
  local y, m, d = tostring(iso):match("^(%d+)-(%d+)-(%d+)")
  if not y then
    return tostring(iso)
  end
  return tonumber(d) .. " " .. MONTHS[tonumber(m)] .. " " .. y
end

-- A title for tabs, previews and the feed: Markdown backticks removed.
local function plain_title(s)
  return (tostring(s):gsub("`", ""))
end

-- Titles in backticks show as code, as in the lesson's own heading.
local function title_html(s)
  return (esc(s):gsub("`([^`]+)`", "<code>%1</code>"))
end

local function type_of(l)
  return TYPES[l.type] or { name = l.type, note = "" }
end

-- `general/<topic>`, `projects/<p>` or `systems/<s>`: the tree group a lesson belongs to.
local function place(l)
  if l.project then
    return "projects/" .. l.project
  elseif l.system then
    return "systems/" .. l.system
  end
  return "general/" .. l.topic
end

local MIME = { png = "image/png", jpg = "image/jpeg", jpeg = "image/jpeg", webp = "image/webp", svg = "image/svg+xml" }

local function file_name(path)
  return path:match("[^/]+$")
end

-- The lesson's Markdown without frontmatter and title, with links to other lessons pointing at their
-- pages. A public lesson's images sit next to its page; a protected lesson's images are embedded, so
-- they stay inside its ciphertext.
local function body(l)
  local text = rs.data.load_frontmatter(l.path).content
  text = text:gsub("^%s*# [^\n]*\n", "", 1)
  for target, url in pairs(l.hrefs or {}) do
    text = text:gsub("%]%(" .. escape_pattern(target) .. "%)", "](" .. url .. ")")
  end
  for target, staged in pairs(l.images or {}) do
    local src
    if l.url:find("^/protected/") then
      local ext = staged:match("%.(%w+)$"):lower()
      src = "data:" .. (MIME[ext] or "application/octet-stream") .. ";base64," .. rs.fs.read(staged .. ".b64")
    else
      src = l.url .. file_name(staged)
    end
    text = text:gsub("%]%(" .. escape_pattern(target) .. "%)", function()
      return "](" .. src .. ")"
    end)
  end
  return text
end

local function unescape(s)
  return (s:gsub("&lt;", "<"):gsub("&gt;", ">"):gsub("&quot;", '"'):gsub("&#39;", "'"):gsub("&amp;", "&"))
end

-- Fenced code with a language gets syntect classes; `static/highlight.css` colors them in both themes.
local function highlight(html)
  return (html:gsub('<pre><code class="language%-([^"]+)">(.-)</code></pre>', function(lang, code)
    return '<pre class="hl"><code class="language-' .. lang .. '">' .. rs.highlight.highlight_sync(unescape(code), lang) .. "</code></pre>"
  end))
end

local function when_lines(l)
  local out = {}
  for k, v in pairs(l.when or {}) do
    if type(v) == "table" then
      v = table.concat(v, ", ")
    end
    table.insert(out, k .. ": " .. tostring(v))
  end
  table.sort(out)
  return out
end

local function by_title(a, b)
  return a.title:lower() < b.title:lower()
end

local function card(l)
  local t = type_of(l)
  return {
    id = l.id,
    title = l.title,
    title_html = title_html(l.title),
    url = l.url,
    type = l.type,
    type_name = t.name,
    topic = l.topic,
    place = place(l),
    verified = l.verified,
    date = date(l.verified),
    stale = l.status == "stale",
  }
end

local function cards(ids, public_only)
  local out = {}
  for _, id in ipairs(ids or {}) do
    local l = by_id[id]
    if l and not (public_only and l.url:find("^/protected/")) then
      table.insert(out, card(l))
    end
  end
  table.sort(out, by_title)
  return out
end

local function newest(list)
  table.sort(list, function(a, b)
    if a.verified ~= b.verified then
      return a.verified > b.verified
    end
    return a.title:lower() < b.title:lower()
  end)
  return list
end

-- The lesson tree: one group per General topic, project and system, lessons by title.
local groups = {}
for _, l in ipairs(site.lessons) do
  local key = place(l)
  local g = groups[key]
  if not g then
    local scope, name = key:match("^(%w+)/(.+)$")
    g = { key = key, scope = scope, name = name, url = "/" .. key .. "/", lessons = {} }
    groups[key] = g
  end
  table.insert(g.lessons, card(l))
end
local tree = { general = {}, projects = {}, systems = {} }
local around = {}
for _, g in pairs(groups) do
  table.sort(g.lessons, by_title)
  g.count = #g.lessons
  for i, c in ipairs(g.lessons) do
    around[c.id] = { prev = g.lessons[i - 1], next = g.lessons[i + 1] }
  end
  table.insert(tree[g.scope], g)
end
for _, list in pairs(tree) do
  table.sort(list, function(a, b)
    return a.name < b.name
  end)
end
local counts = {}
for _, name in ipairs(TYPE_ORDER) do
  counts[name] = 0
end
for _, l in ipairs(site.lessons) do
  counts[l.type] = (counts[l.type] or 0) + 1
end
local type_counts = {}
for _, name in ipairs(TYPE_ORDER) do
  if counts[name] > 0 then
    table.insert(type_counts, { type = name, name = TYPES[name].name, count = counts[name] })
  end
end

local function breadcrumb(l, locked)
  local parts = {}
  if locked then
    table.insert(parts, svg("lock", 12) .. '<a href="/protected/">Protected</a>')
  end
  if l.project then
    table.insert(parts, '<a href="/projects/">Projects</a>')
    table.insert(parts, '<a href="/projects/' .. esc(l.project) .. '/">' .. esc(l.project) .. "</a>")
  elseif l.system then
    table.insert(parts, '<a href="/systems/">Systems</a>')
    table.insert(parts, '<a href="/systems/' .. esc(l.system) .. '/">' .. esc(l.system) .. "</a>")
  else
    table.insert(parts, '<a href="/general/">General</a>')
  end
  if locked or l.project or l.system then
    table.insert(parts, esc(l.topic))
  else
    table.insert(parts, '<a href="/general/' .. esc(l.topic) .. '/">' .. esc(l.topic) .. "</a>")
  end
  return '<nav class="crumbs" aria-label="Breadcrumb">' .. table.concat(parts, '<span aria-hidden="true">›</span>') .. "</nav>"
end

local function links_list(heading, items, empty)
  local out = { '<section class="side-list"><h2 class="label">' .. heading .. "</h2>" }
  if #items == 0 then
    table.insert(out, '<span class="muted">' .. empty .. "</span>")
  end
  for _, c in ipairs(items) do
    table.insert(out, '<a href="' .. c.url .. '">' .. c.title_html .. "</a>")
  end
  table.insert(out, "</section>")
  return table.concat(out)
end

-- A lesson's article and side column. Protected lessons are rendered the same way and then encrypted
-- whole, so nothing about them is outside the ciphertext.
local function lesson_html(l, locked)
  local t = type_of(l)
  local html = highlight(rs.markdown.render(body(l)))
  local toc = {}
  html = html:gsub('<h2 id="([^"]+)">(.-)</h2>', function(id, text)
    table.insert(toc, '<a href="#' .. id .. '">' .. text .. "</a>")
    return '<h2 id="' .. id .. '">' .. text .. '<a class="anchor" href="#' .. id .. '" aria-label="Link to this section">#</a></h2>'
  end)
  local when = when_lines(l)
  local holds = #when > 0 and "<code>" .. table.concat(when, "</code> <code>") .. "</code>" or "anywhere"
  local verified = date(l.verified) .. (HOW[l.verified_how] and (", " .. HOW[l.verified_how]) or "")
  local tags = {}
  for _, tag in ipairs(l.tags or {}) do
    if locked then
      table.insert(tags, '<span class="tag">' .. esc(tag) .. "</span>")
    else
      table.insert(tags, '<a class="tag" href="/tags/' .. slug(tag) .. '/">' .. esc(tag) .. "</a>")
    end
  end
  local c = around[l.id] or {}
  local nav = ""
  if c.prev or c.next then
    local where = l.project or l.system or l.topic
    nav = '<nav class="prevnext" aria-label="In ' .. esc(where) .. '">'
      .. (c.prev and ('<a href="' .. c.prev.url .. '"><span class="muted">← Previous in ' .. esc(where) .. "</span>" .. c.prev.title_html .. "</a>") or "<span></span>")
      .. (c.next and ('<a class="next" href="' .. c.next.url .. '"><span class="muted">Next in ' .. esc(where) .. " →</span>" .. c.next.title_html .. "</a>") or "")
      .. "</nav>"
  end
  local stale = l.status == "stale" and ' <span class="badge stale">Stale</span>' or ""
  local stale_reason = (l.status == "stale" and l.stale_reason)
      and ('<p class="stale-reason"><strong>Stale:</strong> ' .. esc(l.stale_reason) .. "</p>")
    or ""
  local out = {
    '<article class="lesson t-' .. esc(l.type) .. '">',
    '<div class="lesson-head">' .. breadcrumb(l, locked),
    "<h1>" .. title_html(l.title) .. stale .. "</h1>",
    stale_reason,
    '<div class="chips phone-only"><span class="chip type">' .. esc(t.name) .. '</span><span class="chip">Verified ' .. date(l.verified)
      .. '</span><span class="chip">Holds ' .. (#when > 0 and table.concat(when, ", ") or "anywhere") .. "</span></div>",
    "</div>",
    '<div class="type-note">' .. svg(ICONS[l.type] and l.type or "fact", 18) .. "<span><strong>" .. esc(t.name) .. ".</strong> " .. esc(t.note) .. "</span></div>",
    '<div class="prose">' .. html .. "</div>",
    nav,
    "</article>",
    '<aside class="side t-' .. esc(l.type) .. '">',
    #toc > 0 and ('<section class="side-list toc"><h2 class="label">On this page</h2>' .. table.concat(toc) .. "</section>") or "",
    '<dl class="facts">',
    '<div><dt>Type</dt><dd><span class="dot"></span>' .. esc(t.name) .. "</dd></div>",
    "<div><dt>Verified</dt><dd>" .. verified .. "</dd></div>",
    "<div><dt>Holds</dt><dd>" .. holds .. "</dd></div>",
    #tags > 0 and ('<div><dt>Tags</dt><dd class="tags">' .. table.concat(tags) .. "</dd></div>") or "",
    locked and "<div><dt>Access</dt><dd>protected, decrypted in this tab</dd></div>" or "",
    "</dl>",
    links_list("Related", cards(l.related, not locked), "No related lessons."),
    links_list("Linked from", cards(l.backlinks, not locked), "No other lesson links here yet."),
    "</aside>",
  }
  return table.concat(out, "\n")
end

local function plain(l)
  return (rs.html.strip_tags(rs.markdown.render(body(l))):gsub("%s+", " "))
end

-- The lesson's prose as plain text: headings and code blocks left out.
local function prose(l)
  local html = rs.markdown.render(body(l)):gsub("<h%d[^>]*>.-</h%d>", " "):gsub("<pre[^>]*>.-</pre>", " ")
  return (rs.html.strip_tags(html):gsub("%s+", " "))
end

-- The start of the plain text, cut at a word, for link previews.
local function summary(text)
  text = text:gsub("^%s+", "")
  local cut = utf8.offset(text, 161)
  if not cut then
    return text
  end
  return text:sub(1, cut - 1):gsub("%s+%S*$", "") .. "…"
end

local function entry(l)
  return {
    id = l.id,
    title = l.title,
    type = l.type,
    topic = l.topic,
    place = place(l),
    tags = l.tags or {},
    url = l.url,
    text = plain(l),
  }
end

-- Pages carry the tree's groups and only their own group's lessons; `tree.json` has the rest.
local tree_groups = {}
for scope, list in pairs(tree) do
  tree_groups[scope] = {}
  for _, g in ipairs(list) do
    table.insert(tree_groups[scope], { key = g.key, name = g.name, url = g.url, count = g.count })
  end
end

local global = {
  title = title,
  count = #site.lessons,
  tree = tree_groups,
  protected_count = #protected,
  types = type_counts,
  base_url = base_url,
  assets = assets,
  image = base_url ~= "" and (base_url .. "/static/" .. og_image) or "",
  image_builtin = og_image == "og.png",
}

-- Paths of the public pages, for the sitemap, and the verified date of each lesson page.
local indexed = {}
local lastmod = {}

local KIND_URL = { Topic = "/general/", Project = "/projects/", System = "/systems/", Tag = "/tags/", Type = "/types/" }

local function lessons_count(n)
  return n .. (n == 1 and " lesson" or " lessons")
end

-- A lesson's link-preview text: its prose, after its stale reason when it is stale.
local function lesson_summary(l)
  local text = prose(l)
  if l.status == "stale" then
    text = "Stale: " .. (l.stale_reason or "this lesson may no longer hold") .. ". " .. text
  end
  return summary(text)
end

-- `summary` describes the page in link previews; `noindex` keeps it out of search engines and the sitemap.
local function page(path, template, page_title, data, summary_text, noindex)
  page_title = plain_title(page_title)
  data.tree_lessons = groups[data.open or ""] and groups[data.open].lessons or {}
  data.meta = {
    title = page_title,
    description = summary_text or description,
    url = base_url ~= "" and (base_url .. path) or "",
    kind = template == "lesson.html" and "article" or "website",
    noindex = noindex or false,
  }
  if not noindex then
    table.insert(indexed, path)
  end
  return { path = path, template = template, title = page_title, data = data }
end

-- A list of lessons. `kind` (Topic, Project, System, Tag or Type) titles the page `<kind>: <name>`
-- and links to the kind's index; `about` finishes the description after the lesson count.
local function list_page(path, heading, kind, list, open, about)
  local data = { heading = heading, kind = kind, kind_url = KIND_URL[kind], lessons = list, open = open }
  return page(path, "list.html", kind and (kind .. ": " .. heading) or heading, data, lessons_count(#list) .. " " .. about .. ".")
end

-- An index of groups; `more` holds groups shown after the main ones in a compact list.
local function index_page(path, heading, list, desc, more)
  return page(path, "groups.html", heading, { heading = heading, groups = list, more = more or {}, open = "" }, desc)
end

return {
  site = {
    title = title,
    description = description,
    base_url = settings.base_url ~= "" and settings.base_url or "http://localhost:3000",
    author = settings.author or "",
  },

  build = {
    output_dir = "dist",
  },

  data = function()
    return global
  end,

  pages = function()
    indexed = {}
    lastmod = {}
    local recent = {}
    for _, l in ipairs(site.lessons) do
      table.insert(recent, card(l))
    end
    newest(recent)
    local top = {}
    for i = 1, math.min(5, #recent) do
      top[i] = recent[i]
    end
    local topic_cards = {}
    for _, g in ipairs(tree.general) do
      local picks = newest({ table.unpack(g.lessons) })
      table.insert(topic_cards, { name = g.name, url = g.url, count = g.count, lessons = { picks[1], picks[2] } })
    end
    table.sort(topic_cards, function(a, b)
      if a.count ~= b.count then
        return a.count > b.count
      end
      return a.name < b.name
    end)
    local shown = {}
    for i = 1, math.min(6, #topic_cards) do
      shown[i] = topic_cards[i]
    end

    local pages = {
      page("/", "home.html", title, {
        description = settings.description or "",
        cards = shown,
        topic_total = #tree.general,
        recent = top,
        open = "",
      }),
    }
    for _, l in ipairs(site.lessons) do
      lastmod[l.url] = l.verified
      table.insert(pages, page(l.url, "lesson.html", l.title, { main = lesson_html(l, false), open = place(l), current = l.id }, lesson_summary(l)))
      table.insert(pages, page("/lessons/" .. l.id .. "/", "redirect.html", l.title, { to = l.url }, nil, true))
    end
    local ABOUT = { general = "about ", projects = "in the project ", systems = "on the system " }
    for _, g in pairs(groups) do
      local label = g.scope == "general" and "Topic" or (g.scope == "projects" and "Project" or "System")
      table.insert(pages, list_page(g.url, g.name, label, g.lessons, g.key, ABOUT[g.scope] .. g.name))
    end
    for scope, heading in pairs(SCOPES) do
      if #tree[scope] > 0 then
        local list = {}
        for _, g in ipairs(tree[scope]) do
          table.insert(list, { name = g.name, url = g.url, count = g.count })
        end
        local noun = scope == "general" and "topics" or scope
        table.insert(pages, index_page("/" .. scope .. "/", heading, list, #list .. " " .. noun .. " of " .. title .. "."))
      end
    end
    local tag_list, single = {}, {}
    for name, ids in pairs(site.tags or {}) do
      table.insert(pages, list_page("/tags/" .. slug(name) .. "/", name, "Tag", cards(ids, true), "", "tagged " .. name))
      table.insert(#ids > 1 and tag_list or single, { name = name, url = "/tags/" .. slug(name) .. "/", count = #ids })
    end
    local by_name = function(a, b)
      return a.name < b.name
    end
    table.sort(tag_list, by_name)
    table.sort(single, by_name)
    table.insert(pages, index_page("/tags/", "Tags", tag_list, (#tag_list + #single) .. " tags of " .. title .. ".", single))
    local type_list = {}
    for _, name in ipairs(TYPE_ORDER) do
      local list = {}
      for _, l in ipairs(site.lessons) do
        if l.type == name then
          table.insert(list, card(l))
        end
      end
      if #list > 0 then
        table.sort(list, by_title)
        local url = "/types/" .. name .. "/"
        table.insert(pages, list_page(url, TYPES[name].name, "Type", list, "", "of the type " .. TYPES[name].name:lower()))
        table.insert(type_list, { name = TYPES[name].name, url = url, count = #list })
      end
    end
    table.insert(pages, index_page("/types/", "Types", type_list, "Lessons of " .. title .. " by type."))
    table.insert(pages, list_page("/all/", "All lessons", nil, recent, "", "in " .. title .. ", newest first"))
    table.insert(pages, page("/404.html", "404.html", "Page not found", { open = "" }, nil, true))
    if #protected > 0 then
      for _, l in ipairs(protected) do
        table.insert(pages, page(l.url, "protected.html", "Protected lesson", {
          block = rs.crypt.encrypt_html(lesson_html(l, true), { slug = l.id, password = rs.env.get(l.password_env) }),
          open = "protected",
        }, nil, true))
      end
      table.insert(pages, page("/protected/", "protected-index.html", "Protected lessons", { open = "protected" }, nil, true))
    end
    return pages
  end,

  hooks = {
    after_build = function(ctx)
      local out = ctx.output_dir
      for _, f in ipairs({
        "site.css",
        "site.js",
        "theme.js",
        "highlight.css",
        og_image,
        "argon2.umd.min.js",
        "hash-wasm.LICENSE",
        "fonts/Geist-Variable.woff2",
        "fonts/GeistMono-Variable.woff2",
        "fonts/OFL.txt",
      }) do
        rs.fs.copy("static/" .. f, out .. "/static/" .. f)
      end
      local index = {}
      for _, url in pairs(assets) do
        rs.fs.copy("static/" .. file_name(url), out .. url)
      end
      for _, l in ipairs(site.lessons) do
        table.insert(index, entry(l))
        for _, staged in pairs(l.images or {}) do
          rs.fs.copy(staged, out .. l.url .. file_name(staged))
        end
      end
      -- Security headers for Cloudflare Pages and Netlify. The site has no inline script; hash-wasm
      -- compiles WebAssembly for Argon2, and protected images are data: URLs.
      -- `[site] analytics = "cloudflare"`: the Web Analytics beacon Cloudflare injects must load and report.
      local cf = settings.analytics == "cloudflare"
      local headers = {
        "/*",
        "  Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'"
          .. (cf and " https://static.cloudflareinsights.com" or "")
          .. "; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'"
          .. (cf and " https://cloudflareinsights.com" or "")
          .. "; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        "  X-Content-Type-Options: nosniff",
        "  X-Frame-Options: DENY",
        "  Referrer-Policy: strict-origin-when-cross-origin",
        "  Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        "  Cross-Origin-Opener-Policy: same-origin",
        "  Cross-Origin-Resource-Policy: same-origin",
        "/static/v/*",
        "  Cache-Control: public, max-age=31536000, immutable",
        "/protected/*",
        "  X-Robots-Tag: noindex",
      }
      -- With a custom domain, the project's pages.dev addresses stay out of search engines.
      if base_url ~= "" and not base_url:find("%.pages%.dev$") then
        table.insert(headers, "https://:project.pages.dev/*")
        table.insert(headers, "  X-Robots-Tag: noindex")
      end
      table.insert(headers, "")
      rs.fs.write(out .. "/_headers", table.concat(headers, "\n"))
      local tree_json = {}
      for key, g in pairs(groups) do
        tree_json[key] = {}
        for _, c in ipairs(g.lessons) do
          table.insert(tree_json[key], { title = c.title, url = c.url })
        end
      end
      rs.fs.write(out .. "/tree.json", rs.data.to_json(tree_json))
      rs.fs.write(out .. "/search.json", rs.data.to_json(index))
      -- One encrypted list per password, so a password opens only its own lessons. The boxes follow
      -- the order of each password's first lesson and carry no group name.
      if #protected > 0 then
        local lists, order = {}, {}
        for _, l in ipairs(protected) do
          if not lists[l.password_env] then
            lists[l.password_env] = {}
            table.insert(order, l.password_env)
          end
          local e = entry(l)
          e.project = l.project
          e.system = l.system
          table.insert(lists[l.password_env], e)
        end
        local boxes = {}
        for _, var in ipairs(order) do
          table.insert(boxes, rs.crypt.encrypt(rs.data.to_json(lists[var]), rs.env.get(var)))
        end
        rs.fs.write(out .. "/static/protected.json", rs.data.to_json(boxes))
      end
      local base = base_url
      local items = {}
      local all = {}
      for _, l in ipairs(site.lessons) do
        table.insert(all, card(l))
      end
      local rfc = function(d)
        return rs.date.rss_format(rs.date.parse(d))
      end
      for _, c in ipairs(newest(all)) do
        local l = by_id[c.id]
        table.insert(
          items,
          "<item><title>" .. esc((c.stale and "[Stale] " or "") .. plain_title(c.title)) .. "</title><link>" .. base .. c.url
            .. "</link><description>" .. esc(lesson_summary(l)) .. '</description><guid isPermaLink="false">' .. base .. "/lessons/"
            .. c.id .. "/</guid><pubDate>" .. rfc(c.verified) .. "</pubDate></item>"
        )
      end
      local self_link = base ~= ""
          and ('<atom:link href="' .. esc(base .. "/feed.xml") .. '" rel="self" type="application/rss+xml"/>')
        or ""
      local built = #all > 0 and ("<lastBuildDate>" .. rfc(all[1].verified) .. "</lastBuildDate>") or ""
      rs.fs.write(
        out .. "/feed.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom"><channel><title>'
          .. esc(title) .. "</title><link>" .. base .. "/</link><description>" .. esc(description) .. "</description>"
          .. self_link .. built .. table.concat(items) .. "</channel></rss>\n"
      )
      if base ~= "" then
        table.sort(indexed)
        local urls = {}
        for _, path in ipairs(indexed) do
          local mod = lastmod[path] and ("<lastmod>" .. lastmod[path] .. "</lastmod>") or ""
          table.insert(urls, "<url><loc>" .. esc(base .. path) .. "</loc>" .. mod .. "</url>")
        end
        rs.fs.write(
          out .. "/sitemap.xml",
          '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
            .. table.concat(urls) .. "</urlset>\n"
        )
        rs.fs.write(out .. "/robots.txt", "User-agent: *\nAllow: /\nSitemap: " .. base .. "/sitemap.xml\n")
      end
    end,
  },
}
