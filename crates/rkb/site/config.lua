-- The rkb site template for rs-web. `rkb site build` runs rs-web in a folder that holds this file,
-- `templates/`, `site.json` and `lessons/`, which has only the lessons the `web` sink allows.
-- Change it freely after `rkb site init`. Keep lesson pages at the `url` from site.json
-- (`/lessons/<id>/`): the build checks that every page there belongs to a published lesson.
local rs = require("rs-web")

local site = rs.data.load_json("site.json")

local by_id = {}
for _, l in ipairs(site.lessons) do
  by_id[l.id] = l
end
local protected = site.protected or {}
for _, l in ipairs(protected) do
  by_id[l.id] = l
end

local function slug(s)
  return (s:lower():gsub("[^%w]+", "-"):gsub("^-+", ""):gsub("-+$", ""))
end

local function escape_pattern(s)
  return (s:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0"))
end

local function xml(s)
  return (s:gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;"))
end

-- The lesson's Markdown without frontmatter and title, with links to other lessons pointing at their pages.
local function body(l)
  local text = rs.data.load_frontmatter(l.path).content
  text = text:gsub("^%s*# [^\n]*\n", "", 1)
  for target, url in pairs(l.hrefs) do
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

local function cards(ids)
  local out = {}
  for _, id in ipairs(ids) do
    local l = by_id[id]
    if l then
      table.insert(out, { title = l.title, url = l.url, type = l.type, verified = l.verified, topic = l.topic })
    end
  end
  table.sort(out, function(a, b)
    return a.verified > b.verified
  end)
  return out
end

local function groups(kind)
  local out = {}
  for name, ids in pairs(site[kind] or {}) do
    table.insert(out, { name = name, url = "/" .. kind .. "/" .. slug(name) .. "/", count = #ids })
  end
  table.sort(out, function(a, b)
    return a.name < b.name
  end)
  return out
end

local function html_escape(s)
  return (tostring(s):gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;"))
end

-- The whole page of a protected lesson, as HTML to encrypt: nothing about the lesson is outside it.
local function protected_html(l)
  local out = { "<h1>" .. html_escape(l.title) .. "</h1>" }
  local meta = '<p class="meta"><span class="chip">' .. html_escape(l.type) .. "</span> " .. html_escape(l.topic)
  if l.project then
    meta = meta .. " · project " .. html_escape(l.project)
  end
  table.insert(out, meta .. " · verified " .. html_escape(l.verified) .. "</p>")
  local when = when_lines(l)
  if #when > 0 then
    local items = {}
    for _, w in ipairs(when) do
      table.insert(items, "<code>" .. html_escape(w) .. "</code>")
    end
    table.insert(out, '<p class="meta">Holds when: ' .. table.concat(items, " ") .. "</p>")
  end
  table.insert(out, rs.markdown.render(body(l)))
  if #(l.tags or {}) > 0 then
    local tags = {}
    for _, t in ipairs(l.tags) do
      table.insert(tags, '<span class="chip">' .. html_escape(t) .. "</span>")
    end
    table.insert(out, '<p class="chips">' .. table.concat(tags) .. "</p>")
  end
  local related = cards(l.related or {})
  if #related > 0 then
    table.insert(out, '<h2>Related</h2><ul class="cards">')
    for _, r in ipairs(related) do
      table.insert(out, '<li><a href="' .. r.url .. '">' .. html_escape(r.title) .. "</a></li>")
    end
    table.insert(out, "</ul>")
  end
  return table.concat(out, "\n")
end

local all = {}
for _, l in ipairs(site.lessons) do
  table.insert(all, l.id)
end

return {
  site = {
    title = "Lessons learned",
    description = "Notes on problems solved and decisions made.",
    base_url = "http://localhost:3000",
    author = "",
  },

  build = {
    output_dir = "dist",
  },

  pages = function()
    local pages = {
      {
        path = "/",
        template = "home.html",
        title = "Lessons learned",
        data = {
          lessons = cards(all),
          topics = groups("topics"),
          projects = groups("projects"),
          systems = groups("systems"),
          tags = groups("tags"),
          protected_count = #protected,
        },
      },
    }
    if #protected > 0 then
      local index = {}
      for n, l in ipairs(protected) do
        table.insert(pages, {
          path = l.url,
          template = "protected.html",
          title = "Protected lesson",
          data = { block = rs.crypt.encrypt_html(protected_html(l), { slug = l.id }) },
        })
        table.insert(index, { n = n, url = l.url })
      end
      table.insert(pages, { path = "/protected/", template = "protected-index.html", title = "Protected lessons", data = { pages = index } })
    end
    for _, l in ipairs(site.lessons) do
      local tags = {}
      for _, t in ipairs(l.tags or {}) do
        table.insert(tags, { name = t, url = "/tags/" .. slug(t) .. "/" })
      end
      table.insert(pages, {
        path = l.url,
        template = "lesson.html",
        title = l.title,
        content = body(l),
        data = {
          lesson = l,
          when = when_lines(l),
          tags = tags,
          related = cards(l.related or {}),
          topic_url = "/topics/" .. slug(l.topic) .. "/",
        },
      })
    end
    for _, kind in ipairs({ "topics", "projects", "systems", "tags" }) do
      for name, ids in pairs(site[kind] or {}) do
        table.insert(pages, {
          path = "/" .. kind .. "/" .. slug(name) .. "/",
          template = "list.html",
          title = name,
          data = { kind = kind:sub(1, -2), lessons = cards(ids) },
        })
      end
    end
    return pages
  end,

  hooks = {
    after_build = function(ctx)
      for _, f in ipairs({ "argon2.umd.min.js", "decrypt.js", "hash-wasm.LICENSE" }) do
        rs.fs.write(ctx.output_dir .. "/static/" .. f, rs.fs.read("static/" .. f))
      end
      local items = {}
      for _, c in ipairs(cards(all)) do
        table.insert(
          items,
          "<item><title>" .. xml(c.title) .. "</title><link>" .. ctx.base_url .. c.url .. "</link><guid>" .. ctx.base_url
            .. c.url .. "</guid><pubDate>" .. rs.date.rss_format(rs.date.parse(c.verified)) .. "</pubDate></item>"
        )
      end
      rs.fs.write(
        ctx.output_dir .. "/feed.xml",
        '<?xml version="1.0" encoding="UTF-8"?>\n<rss version="2.0"><channel><title>Lessons learned</title><link>'
          .. ctx.base_url .. "/</link><description>Lessons learned</description>" .. table.concat(items) .. "</channel></rss>\n"
      )
    end,
  },
}
