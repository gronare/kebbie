if vim.fn.has("nvim-0.10") == 0 then
  vim.api.nvim_buf_set_lines(0, 0, -1, false, { "kebbie's review needs nvim 0.10 or newer; this is " .. tostring(vim.version()) .. "." })
  return
end

local dir = vim.env.REVIEW_DIR
local herdr = (vim.env.HERDR_BIN_PATH or "") ~= "" and vim.env.HERDR_BIN_PATH or "herdr"
local marks = vim.api.nvim_create_namespace("review")
local shown = { path = "", title = "", sha = "", short = "", index = 0, count = 0, diff = {} }
local numbers = {}
local sent = 0
local stale = false
local float_win
-- Only saved buffers count as the user's edits, so files Claude changes on disk aren't reported back to it.
local baseline = {}
local saved = {}

local function read_file(name)
  local file = io.open(dir .. "/" .. name)
  if not file then return nil end
  local text = file:read("*a")
  file:close()
  return vim.trim(text)
end

local agent_pane = read_file("agent")

-- herdr hands a pane's OSC 52 to the terminal it runs in, so with no clipboard tool (a devbox over SSH) a copy still reaches the desktop; herdr answers no clipboard read, so paste stays in nvim.
local function clipboard_fallback()
  if vim.g.clipboard ~= nil or vim.fn["provider#clipboard#Executable"]() ~= "" then return end
  local osc52 = require("vim.ui.clipboard.osc52")
  local function paste() return { vim.split(vim.fn.getreg('"'), "\n"), vim.fn.getregtype('"') } end
  vim.g.clipboard = { name = "OSC 52", copy = { ["+"] = osc52.copy("+"), ["*"] = osc52.copy("+") }, paste = { ["+"] = paste, ["*"] = paste } }
  vim.g.loaded_clipboard_provider = nil
  vim.cmd("runtime autoload/provider/clipboard.vim")
end
clipboard_fallback()

vim.cmd.enew()
local diff_buf = vim.api.nvim_get_current_buf()
local diff_win = vim.api.nvim_get_current_win()

-- Captured before the diff changes them, so the float shows files with the user's own options.
local float_options = {}
for _, option in ipairs({ "number", "relativenumber", "signcolumn", "statuscolumn", "winbar", "statusline", "wrap", "breakindent", "showbreak", "linebreak" }) do
  float_options[option] = vim.api.nvim_get_option_value(option, { win = diff_win })
end

vim.bo.buftype = "nofile"
vim.bo.bufhidden = "hide"
vim.bo.swapfile = false
vim.bo.modifiable = false
vim.bo.filetype = "diff"
vim.o.mouse = "a"
if vim.o.laststatus ~= 2 then vim.o.laststatus = 2 end
vim.wo.number = false
vim.wo.relativenumber = false
vim.wo.signcolumn = "yes"
vim.wo.wrap = true
vim.wo.breakindent = true
vim.wo.showbreak = "↪ "
vim.wo.linebreak = true
vim.o.sidescroll = 1

local function number_lines()
  numbers = {}
  local old, new
  for lnum, line in ipairs(vim.api.nvim_buf_get_lines(diff_buf, 0, -1, false)) do
    local old_start, new_start = line:match("^@@ %-(%d+),?%d* %+(%d+)")
    if old_start then
      old, new = tonumber(old_start), tonumber(new_start)
    elseif old and line:match("^%-") then
      numbers[lnum] = { old = old }
      old = old + 1
    elseif old and line:match("^%+") then
      numbers[lnum] = { new = new }
      new = new + 1
    elseif old and line:match("^ ") then
      numbers[lnum] = { old = old, new = new }
      old, new = old + 1, new + 1
    end
  end
end

KebbieReview = {}

function KebbieReview.column()
  local number = numbers[vim.v.lnum]
  if vim.v.virtnum ~= 0 or not number then return "%s     " end
  return "%s%#LineNr#" .. string.format("%4s ", number.new or number.old) .. "%*"
end

local function span(low, high)
  return low == high and tostring(low) or (low .. "-" .. high)
end

local function diff_location(first, last)
  local function side_span(side)
    local low, high
    for lnum = first, last do
      local n = numbers[lnum] and numbers[lnum][side]
      if n then low, high = low or n, n end
    end
    if low then return span(low, high) end
  end
  local within = shown.sha ~= "" and (" in " .. shown.short) or ""
  local new = side_span("new")
  if new then return shown.path .. ":" .. new .. within end
  local old = side_span("old")
  if old then return shown.path .. ":" .. old .. " (before)" .. within end
  return shown.path .. within
end

-- :. leaves the repo root absolute or empty.
local function relative(buf)
  local name = vim.api.nvim_buf_get_name(buf)
  local rel = vim.fn.fnamemodify(name, ":.")
  return (rel == "" or (rel == name and vim.fn.isdirectory(name) == 1)) and "./" or rel
end

local function is_real_file(buf)
  return vim.bo[buf].buftype == "" and vim.api.nvim_buf_get_name(buf) ~= ""
end

local function update_bars()
  local where = shown.path == "" and "nothing to show" or (shown.path .. "  %#Comment#· " .. shown.title .. (shown.sha ~= "" and (" · " .. shown.short) or "") .. "%*")
  local right = stale and "%#DiagnosticWarn#changed on disk · u reloads%*  " or ""
  local count = shown.count > 0 and (shown.index .. "/" .. shown.count) or ""
  vim.wo[diff_win].winbar = " " .. where .. "%=" .. right .. "%#Comment#" .. count .. "%* "
  local note = sent > 0 and ("  ·  " .. sent .. " sent to Claude") or ""
  vim.wo[diff_win].statusline = " %#Comment#drag selects · c comments · y copies · o edits the file" .. note .. "%*%=%#Comment#" .. (vim.wo[diff_win].wrap and "wrap" or "nowrap") .. "%* "
end

local function herdr_call(args)
  local result = vim.system(vim.list_extend({ herdr }, args), { text = true }):wait()
  if result.code ~= 0 then
    vim.notify("herdr " .. args[1] .. " " .. args[2] .. " failed: " .. (result.stderr or ""), vim.log.levels.ERROR)
  end
  return result.code == 0
end

local function send_to_claude(text)
  if not agent_pane or agent_pane == "" then
    vim.notify("No Claude pane to send to", vim.log.levels.WARN)
    return false
  end
  return herdr_call({ "pane", "send-text", agent_pane, text .. "\n" })
end

-- The view moves the focus, since only it knows which side of it nvim sits on.
local function to_view(key)
  local view = read_file("pane")
  if view then herdr_call({ "pane", "send-keys", view, key }) end
end

local function show_sent(buf, last, text, ok)
  local label = ok and ("  ✓ sent to Claude: " .. text) or ("  ✗ not sent: " .. text)
  vim.api.nvim_buf_set_extmark(buf, marks, last - 1, 0, {
    sign_text = ok and "✓" or "✗",
    sign_hl_group = ok and "DiagnosticOk" or "DiagnosticError",
    virt_lines = { { { label, ok and "DiagnosticOk" or "DiagnosticError" } } },
  })
end

local function prompt(title, footer, on_submit)
  local buf = vim.api.nvim_create_buf(false, true)
  vim.b[buf].copilot_enabled = false
  local width = math.min(90, vim.o.columns - 8)
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor",
    width = width,
    height = 3,
    row = math.floor((vim.o.lines - 5) / 2),
    col = math.floor((vim.o.columns - width) / 2),
    style = "minimal",
    border = "rounded",
    title = " " .. title .. " ",
    title_pos = "center",
    footer = " " .. footer .. " ",
    footer_pos = "center",
    zindex = 100,
  })
  vim.wo[win].wrap = true
  pcall(function() require("cmp").setup.buffer({ enabled = false }) end)

  local function close()
    vim.cmd.stopinsert()
    if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end
  vim.keymap.set({ "i", "n" }, "<CR>", function()
    local text = vim.trim(table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), " "))
    close()
    if text ~= "" then on_submit(text) end
  end, { buffer = buf })
  vim.keymap.set({ "i", "n" }, "<Esc>", close, { buffer = buf })
  vim.cmd.startinsert()
end

local function comment(first, last)
  local buf = vim.api.nvim_get_current_buf()
  local where
  if buf == diff_buf then
    if shown.path == "" then return end
    where = diff_location(first, last)
  elseif is_real_file(buf) then
    where = relative(buf) .. ":" .. span(first, last)
  else
    vim.notify("Comments work in the diff and in files", vim.log.levels.WARN)
    return
  end
  prompt("comment on " .. where, "⏎ type it into Claude · esc cancel", function(text)
    local ok = send_to_claude(where .. " — " .. text)
    show_sent(buf, last, text, ok)
    if ok then
      sent = sent + 1
      update_bars()
    end
  end)
end

local function comment_on_line()
  local line = vim.fn.line(".")
  comment(line, line)
end

-- Visual mode ends here and now: an Esc left in the typeahead would land in the comment box and drop it out of insert mode.
local function comment_on_selection()
  local first, last = vim.fn.line("v"), vim.fn.line(".")
  if first > last then first, last = last, first end
  vim.api.nvim_feedkeys(vim.keycode("<Esc>"), "nx", false)
  comment(first, last)
end

local function unsaved()
  local bufs = {}
  for buf in pairs(baseline) do
    if vim.api.nvim_buf_is_valid(buf) and vim.bo[buf].modified then table.insert(bufs, buf) end
  end
  return bufs
end

local function guard(action)
  local bufs = unsaved()
  if #bufs == 0 then return action() end
  local names = vim.tbl_map(relative, bufs)
  local buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.list_extend({ "", " Unsaved changes in:" }, vim.tbl_map(function(name) return "   " .. name end, names)))
  local width = math.min(70, vim.o.columns - 8)
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor",
    width = width,
    height = #names + 3,
    row = math.floor((vim.o.lines - #names - 5) / 2),
    col = math.floor((vim.o.columns - width) / 2),
    style = "minimal",
    border = "rounded",
    title = " Unsaved changes ",
    title_pos = "center",
    footer = " s: save  ·  d: discard  ·  esc: keep editing ",
    footer_pos = "center",
    zindex = 100,
  })
  local function close() vim.api.nvim_win_close(win, true) end
  vim.keymap.set("n", "s", function()
    close()
    for _, b in ipairs(bufs) do vim.api.nvim_buf_call(b, function() vim.cmd("silent write") end) end
    action()
  end, { buffer = buf })
  vim.keymap.set("n", "d", function()
    close()
    for _, b in ipairs(bufs) do vim.api.nvim_buf_call(b, function() vim.cmd("silent edit!") end) end
    action()
  end, { buffer = buf })
  vim.keymap.set("n", "<Esc>", close, { buffer = buf })
end

local function report_edits()
  local reviewed_changed = false
  for buf, before in pairs(baseline) do
    if saved[buf] and vim.api.nvim_buf_is_valid(buf) and not vim.bo[buf].modified then
      saved[buf] = nil
      local after = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
      local hunks = vim.diff(table.concat(before, "\n") .. "\n", table.concat(after, "\n") .. "\n", { result_type = "indices" })
      if #hunks > 0 then
        local ranges = vim.tbl_map(function(h)
          local start, count = h[3], h[4]
          return count == 0 and (math.max(start, 1) .. " (removed lines)") or span(start, start + count - 1)
        end, hunks)
        local name = relative(buf)
        if send_to_claude(name .. ":" .. table.concat(ranges, ", ") .. " — edited by me during the review") then
          baseline[buf] = after
          reviewed_changed = reviewed_changed or name == shown.path
          vim.notify("Told Claude about your edit to " .. vim.fn.fnamemodify(name, ":t"))
        end
      end
    end
  end
  return reviewed_changed
end

local function diff_lines(stdout)
  local lines = vim.split(stdout, "\n", { trimempty = true })
  for index, line in ipairs(lines) do
    if line:match("^@@") then return vim.list_slice(lines, index) end
  end
  return lines
end

-- A diff that finishes after a newer shown.json was loaded is dropped, so the buffer always holds the file the winbar names.
local function run_diff(on_done)
  if #shown.diff == 0 then return on_done({}) end
  local asked = shown
  vim.system(asked.diff, { text = true }, vim.schedule_wrap(function(result)
    if asked ~= shown then return end
    on_done(diff_lines(result.stdout or ""))
  end))
end

local function set_diff(lines)
  vim.bo[diff_buf].modifiable = true
  vim.api.nvim_buf_set_lines(diff_buf, 0, -1, false, lines)
  vim.bo[diff_buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(diff_buf, marks, 0, -1)
  number_lines()
  stale = false
  update_bars()
end

function KebbieReview.load()
  local text = read_file("shown.json")
  local ok, decoded = pcall(vim.json.decode, text or "")
  if not ok or type(decoded) ~= "table" then return 0 end
  shown = decoded
  run_diff(function(lines)
    set_diff(lines)
    if vim.api.nvim_win_is_valid(diff_win) then vim.api.nvim_win_set_cursor(diff_win, { 1, 0 }) end
  end)
  return 1
end

local function refresh_diff()
  run_diff(set_diff)
end

local function view_alive()
  local pid = tonumber(read_file("live") or "")
  return pid ~= nil and vim.uv.kill(pid, 0) == 0
end

vim.uv.new_timer():start(3000, 3000, vim.schedule_wrap(function()
  if not view_alive() then return vim.cmd("qa!") end
  if shown.sha ~= "" then return end
  run_diff(function(lines)
    if not vim.api.nvim_buf_is_valid(diff_buf) then return end
    local changed = not vim.deep_equal(lines, vim.api.nvim_buf_get_lines(diff_buf, 0, -1, false))
    if changed ~= stale then
      stale = changed
      update_bars()
    end
  end)
end))

local function leader_name()
  local leader = vim.g.mapleader or "\\"
  return leader == " " and "space" or leader
end

local function float_config(buf)
  local width = math.floor(vim.o.columns * 0.85)
  local height = math.floor((vim.o.lines - 2) * 0.85)
  return {
    relative = "editor",
    width = width,
    height = height,
    row = math.floor((vim.o.lines - height) / 2) - 1,
    col = math.floor((vim.o.columns - width) / 2),
    border = "rounded",
    title = " " .. (is_real_file(buf) and relative(buf) or "files") .. " ",
    title_pos = "center",
    footer = " ctrl-q: back to the diff  ·  " .. leader_name() .. " m: comment ",
    footer_pos = "center",
  }
end

local function reset_window_options(win)
  for option, value in pairs(float_options) do
    vim.api.nvim_set_option_value(option, value, { win = win })
  end
end

local function open_in_float(target, line)
  if not (float_win and vim.api.nvim_win_is_valid(float_win)) then
    float_win = vim.api.nvim_open_win(diff_buf, true, float_config(diff_buf))
    reset_window_options(float_win)
  else
    vim.api.nvim_set_current_win(float_win)
  end
  vim.cmd.edit(vim.fn.fnameescape(target))
  -- A buffer restores the window options it last had.
  reset_window_options(float_win)
  if line then pcall(vim.api.nvim_win_set_cursor, float_win, { line, 0 }) end
  vim.cmd("normal! zz")
end

local function close_float()
  if not (float_win and vim.api.nvim_win_is_valid(float_win)) then return end
  guard(function()
    vim.cmd.stopinsert()
    if float_win and vim.api.nvim_win_is_valid(float_win) then vim.api.nvim_win_close(float_win, true) end
  end)
end

local function file_line()
  local lnum = vim.fn.line(".")
  for l = lnum, vim.api.nvim_buf_line_count(diff_buf) do
    if numbers[l] and numbers[l].new then return numbers[l].new end
  end
  for l = lnum, 1, -1 do
    if numbers[l] and numbers[l].new then return numbers[l].new end
  end
  return 1
end

local function open_file()
  if vim.fn.filereadable(shown.path) == 0 then
    vim.notify(shown.path .. " no longer exists", vim.log.levels.WARN)
    return
  end
  open_in_float(shown.path, file_line())
end

-- Configs that lazy-load neo-tree on a key have no :Neotree until lazy loads it.
local function neo_tree()
  if vim.fn.exists(":Neotree") ~= 2 then
    pcall(function() require("lazy").load({ plugins = { "neo-tree.nvim" } }) end)
  end
  return vim.fn.exists(":Neotree") == 2
end

local function open_folder()
  local folder = vim.fn.fnamemodify(shown.path, ":h")
  if vim.fn.filereadable(shown.path) == 1 and neo_tree() then
    -- Neo-tree opens no float from inside another float.
    vim.api.nvim_set_current_win(diff_win)
    vim.cmd("Neotree float reveal_file=" .. vim.fn.fnameescape(vim.fn.fnamemodify(shown.path, ":p")))
  else
    open_in_float(folder)
  end
end

vim.api.nvim_create_autocmd("BufReadPost", {
  callback = function(event)
    if is_real_file(event.buf) and not baseline[event.buf] then
      baseline[event.buf] = vim.api.nvim_buf_get_lines(event.buf, 0, -1, false)
    end
  end,
})

vim.api.nvim_create_autocmd("BufWritePost", {
  callback = function(event)
    if baseline[event.buf] then saved[event.buf] = true end
  end,
})

-- Neo-tree opens files in the last regular window, which is the diff's.
vim.api.nvim_create_autocmd("BufWinEnter", {
  callback = function(event)
    if vim.api.nvim_get_current_win() ~= diff_win or event.buf == diff_buf or not is_real_file(event.buf) then return end
    local line = vim.api.nvim_win_get_cursor(diff_win)[1]
    vim.schedule(function()
      vim.api.nvim_win_set_buf(diff_win, diff_buf)
      open_in_float(vim.api.nvim_buf_get_name(event.buf), line)
    end)
  end,
})

vim.api.nvim_create_autocmd("BufWinEnter", {
  callback = function(event)
    if float_win and vim.api.nvim_get_current_win() == float_win then
      local config = float_config(event.buf)
      vim.api.nvim_win_set_config(float_win, { title = config.title, title_pos = "center" })
    end
  end,
})

vim.api.nvim_create_autocmd("WinClosed", {
  callback = function(event)
    if float_win and tonumber(event.match) == float_win then
      float_win = nil
      vim.schedule(function()
        if report_edits() then refresh_diff() end
      end)
    end
  end,
})

local function map(modes, lhs, rhs, desc, global)
  vim.keymap.set(modes, lhs, rhs, { buffer = not global and diff_buf or nil, desc = desc })
end

map("n", "c", comment_on_line, "Comment to Claude")
map("x", "c", comment_on_selection, "Comment to Claude")
map("n", "<leader>m", comment_on_line, "Comment to Claude", true)
map("x", "<leader>m", comment_on_selection, "Comment to Claude", true)
map({ "n", "i", "x", "s" }, "<C-q>", close_float, "Back to the diff", true)
map("n", "o", open_file, "Edit the file over the diff")
map("n", "-", open_folder, "Open the file's folder over the diff")
map("n", "u", refresh_diff, "Reload the diff")
map("n", "w", function() vim.wo[diff_win].wrap = not vim.wo[diff_win].wrap; update_bars() end, "Toggle line wrap")
map("n", "<Tab>", function() to_view("tab") end, "Back to the steps")
map("n", "<S-Tab>", function() to_view("shift+tab") end, "Back to the files")
map("n", "q", function() guard(function() report_edits(); to_view("q") end) end, "Close the review")
map("x", "y", '"+y', "Copy to the clipboard")
map("n", "yy", '"+yy', "Copy the line to the clipboard")
vim.api.nvim_create_autocmd("TextYankPost", {
  buffer = diff_buf,
  callback = function()
    if vim.v.event.regname == "+" then vim.notify("Copied " .. #vim.v.event.regcontents .. " line(s)") end
  end,
})

-- This nvim is the review's alone, and a statusline plugin would draw over its keys.
local function own_statusline()
  pcall(function() require("lualine").hide({ place = { "statusline" }, unhide = false }) end)
  update_bars()
end
vim.api.nvim_create_autocmd("User", { pattern = "VeryLazy", callback = own_statusline })
vim.schedule(own_statusline)

vim.wo[diff_win].statuscolumn = "%{%v:lua.KebbieReview.column()%}"
update_bars()
KebbieReview.load()
