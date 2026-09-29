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

-- The lesson's Markdown without frontmatter and title, with links to other lessons pointing at their pages.
local function body(l)
  local text = rs.data.load_frontmatter(l.path).content
  text = text:gsub("^%s*# [^\n]*\n", "", 1)
  for target, url in pairs(l.hrefs or {}) do
    text = text:gsub("%]%(" .. escape_pattern(target) .. "%)", "](" .. url .. ")")
  end
  return text
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
  local html = rs.markdown.render(body(l))
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
  local out = {
    '<article class="lesson t-' .. esc(l.type) .. '">',
    '<div class="lesson-head">' .. breadcrumb(l, locked),
    "<h1>" .. title_html(l.title) .. stale .. "</h1>",
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

-- ponytail: every page repeats the whole tree, so output grows with lessons squared; past a few
-- thousand lessons, write the tree to a JSON file and let site.js fill it.
local global = {
  title = title,
  count = #site.lessons,
  tree = tree,
  protected_count = #protected,
  types = type_counts,
}

local function page(path, template, page_title, data)
  return { path = path, template = template, title = page_title, data = data }
end

local function list_page(path, heading, kind, list, open)
  return page(path, "list.html", heading, { heading = heading, kind = kind, lessons = list, open = open })
end

local function index_page(path, heading, list)
  return page(path, "groups.html", heading, { heading = heading, groups = list, open = "" })
end

return {
  site = {
    title = title,
    description = settings.description ~= "" and settings.description or "Notes on problems solved and decisions made.",
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
      table.insert(pages, page(l.url, "lesson.html", l.title, { main = lesson_html(l, false), open = place(l), current = l.id }))
      table.insert(pages, page("/lessons/" .. l.id .. "/", "redirect.html", l.title, { to = l.url }))
    end
    for _, g in pairs(groups) do
      local label = g.scope == "general" and "Topic" or (g.scope == "projects" and "Project" or "System")
      table.insert(pages, list_page(g.url, g.name, label, g.lessons, g.key))
    end
    for scope, heading in pairs(SCOPES) do
      if #tree[scope] > 0 then
        local list = {}
        for _, g in ipairs(tree[scope]) do
          table.insert(list, { name = g.name, url = g.url, count = g.count })
        end
        table.insert(pages, index_page("/" .. scope .. "/", heading, list))
      end
    end
    local tag_list = {}
    for name, ids in pairs(site.tags or {}) do
      table.insert(pages, list_page("/tags/" .. slug(name) .. "/", name, "Tag", cards(ids, true), ""))
      table.insert(tag_list, { name = name, url = "/tags/" .. slug(name) .. "/", count = #ids })
    end
    table.sort(tag_list, function(a, b)
      return a.name < b.name
    end)
    table.insert(pages, index_page("/tags/", "Tags", tag_list))
    if #protected > 0 then
      for _, l in ipairs(protected) do
        table.insert(pages, page(l.url, "protected.html", "Protected lesson", {
          block = rs.crypt.encrypt_html(lesson_html(l, true), { slug = l.id, password = rs.env.get(l.password_env) }),
          open = "protected",
        }))
      end
      table.insert(pages, page("/protected/", "protected-index.html", "Protected lessons", { open = "protected" }))
    end
    return pages
  end,

  hooks = {
    after_build = function(ctx)
      local out = ctx.output_dir
      for _, f in ipairs({
        "site.css",
        "site.js",
        "argon2.umd.min.js",
        "hash-wasm.LICENSE",
        "fonts/Geist-Variable.woff2",
        "fonts/GeistMono-Variable.woff2",
        "fonts/OFL.txt",
      }) do
        rs.fs.copy("static/" .. f, out .. "/static/" .. f)
      end
      local index = {}
      for _, l in ipairs(site.lessons) do
        table.insert(index, entry(l))
      end
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
      local base = (settings.base_url ~= nil and settings.base_url ~= "") and settings.base_url:gsub("/+$", "") or ctx.base_url
      local items = {}
      local all = {}
      for _, l in ipairs(site.lessons) do
        table.insert(all, card(l))
      end
      for _, c in ipairs(newest(all)) do
        table.insert(
          items,
          "<item><title>" .. esc(c.title) .. "</title><link>" .. base .. c.url .. "</link><guid>" .. base .. "/lessons/" .. c.id
            .. "/</guid><pubDate>" .. rs.date.rss_format(rs.date.parse(c.verified)) .. "</pubDate></item>"
        )
      end
      rs.fs.write(
        out .. "/feed.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n<rss version="2.0"><channel><title>' .. esc(title) .. "</title><link>" .. base
          .. "/</link><description>" .. esc(settings.description ~= "" and settings.description or title) .. "</description>"
          .. table.concat(items) .. "</channel></rss>\n"
      )
    end,
  },
}
