-- Loaded by review.sh on a unified diff read from stdin. In the diff: c comments, o opens the real file in a float over
-- the diff, - opens its folder there, r marks the file read, q or Esc goes back to the list. Anywhere: <leader>m
-- comments, ctrl-q closes the float. Comments are typed into the Claude pane's input box as "path:line — text"
-- (not submitted), and files saved from the float are reported there too.
local agent_pane = vim.env.REVIEW_AGENT_PANE
local path = vim.env.REVIEW_PATH
local diff_buf = vim.api.nvim_get_current_buf()
local diff_win = vim.api.nvim_get_current_win()
local marks = vim.api.nvim_create_namespace("review")
local sent = 0
local float_win
-- Real files opened in the float: buffer -> lines as last reported to Claude (as read from disk at first), and the
-- buffers saved since. Only saves count as your edits, so a file Claude changes on disk is not reported back to it.
local baseline = {}
local saved = {}

-- The window options the diff changes below, as the user's config left them, for the float to show files as usual.
local float_options = {}
for _, option in ipairs({ "number", "relativenumber", "signcolumn", "winbar", "wrap", "breakindent", "showbreak", "linebreak" }) do
  float_options[option] = vim.api.nvim_get_option_value(option, { win = diff_win })
end

vim.bo.buftype = "nofile"
vim.bo.modifiable = false
vim.bo.filetype = "diff"
vim.wo.number = false
vim.wo.relativenumber = false
vim.wo.signcolumn = "yes"
-- Wrapped continuations are indented and marked so they don't read as context lines; unwrapped, scroll a column at a time.
vim.wo.breakindent = true
vim.wo.showbreak = "↪ "
vim.wo.linebreak = true
vim.o.sidescroll = 1

-- Diff buffer line -> { old = n } for removed lines, { new = n } for added, both for context lines.
local numbers
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
number_lines()

local function span(low, high)
  return low == high and tostring(low) or (low .. "-" .. high)
end

-- path:12, path:12-14, or path:7 (before) when the selection only covers removed lines.
local function diff_location(first, last)
  local function side_span(side)
    local low, high
    for lnum = first, last do
      local n = numbers[lnum] and numbers[lnum][side]
      if n then low, high = low or n, n end
    end
    if low then return span(low, high) end
  end
  local new = side_span("new")
  if new then return path .. ":" .. new end
  local old = side_span("old")
  if old then return path .. ":" .. old .. " (before)" end
  return path
end

-- Repo-relative; the repo root itself, which :. leaves absolute or empty, is ./.
local function relative(buf)
  local name = vim.api.nvim_buf_get_name(buf)
  local rel = vim.fn.fnamemodify(name, ":.")
  return (rel == "" or (rel == name and vim.fn.isdirectory(name) == 1)) and "./" or rel
end

local function is_real_file(buf)
  return vim.bo[buf].buftype == "" and vim.api.nvim_buf_get_name(buf) ~= ""
end

-- Set when the file's diff no longer matches the buffer; u reloads it.
local stale = false

local function update_winbar()
  local note = sent > 0 and ("  ·  " .. sent .. " sent to Claude") or ""
  local keys = stale and "%#DiagnosticWarn#changed on disk  ·  u reloads%* " or "o open  ·  - folder  ·  c comment  ·  w wrap  ·  r read  ·  q back "
  vim.wo[diff_win].winbar = " " .. path .. note .. "  %=" .. keys
end

local function send_to_claude(text)
  local result = vim.system({ "herdr", "pane", "send-text", agent_pane, text .. "\n" }):wait()
  if result.code ~= 0 then
    vim.notify("herdr send-text failed: " .. (result.stderr or ""), vim.log.levels.ERROR)
  end
  return result.code == 0
end

local function show_sent(buf, last, text, ok)
  local label = ok and ("  ✓ sent: " .. text) or ("  ✗ not sent: " .. text)
  vim.api.nvim_buf_set_extmark(buf, marks, last - 1, 0, {
    sign_text = ok and "✓" or "✗",
    sign_hl_group = ok and "DiagnosticOk" or "DiagnosticError",
    virt_lines = { { { label, ok and "DiagnosticOk" or "DiagnosticError" } } },
  })
end

-- A small centred box over everything: Enter submits the text, Esc cancels.
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

-- Comments on lines first..last of the current buffer: the diff, or a real file opened in the float.
local function comment(first, last)
  if not agent_pane or agent_pane == "" then
    vim.notify("No Claude pane to send to", vim.log.levels.WARN)
    return
  end
  local buf = vim.api.nvim_get_current_buf()
  local where
  if buf == diff_buf then
    where = diff_location(first, last)
  elseif is_real_file(buf) then
    where = relative(buf) .. ":" .. span(first, last)
  else
    vim.notify("Comments work in the diff and in files", vim.log.levels.WARN)
    return
  end
  prompt(where, "enter: send to Claude  ·  esc: cancel", function(text)
    local ok = send_to_claude(where .. " — " .. text)
    show_sent(buf, last, text, ok)
    if ok then
      sent = sent + 1
      update_winbar()
    end
  end)
end

local function comment_on_line()
  local line = vim.fn.line(".")
  comment(line, line)
end

local function comment_on_selection()
  local first, last = vim.fn.line("v"), vim.fn.line(".")
  if first > last then first, last = last, first end
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<Esc>", true, false, true), "n", false)
  comment(first, last)
end

-- Unsaved edits block closing the float or leaving the review until saved or discarded.
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

-- "path:3-5, 12 — edited by me during the review" for each file saved with changes since the last report.
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
          reviewed_changed = reviewed_changed or name == path
          vim.notify("Told Claude about your edit to " .. vim.fn.fnamemodify(name, ":t"))
        end
      end
    end
  end
  return reviewed_changed
end

-- Reloads the diff after the reviewed file changed; the sent markers go, since their lines moved.
local function refresh_diff()
  local result = vim.system({ "sh", vim.env.REVIEW_SCRIPT, "diff", vim.env.REVIEW_BASE, path }, { text = true }):wait()
  if result.code ~= 0 then return end
  local lines = vim.split(result.stdout, "\n", { trimempty = true })
  vim.bo[diff_buf].modifiable = true
  vim.api.nvim_buf_set_lines(diff_buf, 0, -1, false, lines)
  vim.bo[diff_buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(diff_buf, marks, 0, -1)
  number_lines()
  stale = false
  update_winbar()
end

-- Checks every 3 s whether the diff on disk moved on, without touching what is on screen.
local function diff_lines(stdout)
  return vim.split(stdout, "\n", { trimempty = true })
end

vim.uv.new_timer():start(3000, 3000, vim.schedule_wrap(function()
  vim.system({ "sh", vim.env.REVIEW_SCRIPT, "diff", vim.env.REVIEW_BASE, path }, { text = true }, vim.schedule_wrap(function(result)
    if result.code ~= 0 or not vim.api.nvim_buf_is_valid(diff_buf) then return end
    local changed = not vim.deep_equal(diff_lines(result.stdout), vim.api.nvim_buf_get_lines(diff_buf, 0, -1, false))
    if changed ~= stale then
      stale = changed
      update_winbar()
    end
  end))
end))

-- How the footer names <leader>, which differs per config.
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

-- Opens target (a file or a folder) in the float, creating it over the diff when needed.
local function open_in_float(target, line)
  if not (float_win and vim.api.nvim_win_is_valid(float_win)) then
    float_win = vim.api.nvim_open_win(diff_buf, true, float_config(diff_buf))
    reset_window_options(float_win)
  else
    vim.api.nvim_set_current_win(float_win)
  end
  vim.cmd.edit(vim.fn.fnameescape(target))
  -- A buffer brings back the window options it last had, the diff window's if it passed through there.
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

-- The file line for the diff line under the cursor: removed lines take the next line that still exists.
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
  if vim.fn.filereadable(path) == 0 then
    vim.notify(path .. " no longer exists", vim.log.levels.WARN)
    return
  end
  open_in_float(path, file_line())
end

-- Neo-tree's own float when it is installed, else whatever handles folders (netrw by default) inside the review float.
-- Configs that lazy-load neo-tree on a key have no :Neotree command until lazy.nvim loads it.
local function neo_tree()
  if vim.fn.exists(":Neotree") ~= 2 then
    pcall(function() require("lazy").load({ plugins = { "neo-tree.nvim" } }) end)
  end
  return vim.fn.exists(":Neotree") == 2
end

local function open_folder()
  local folder = vim.fn.fnamemodify(path, ":h")
  if vim.fn.filereadable(path) == 1 and neo_tree() then
    -- Neo-tree opens no float from inside another float, so it starts from the diff window.
    vim.api.nvim_set_current_win(diff_win)
    vim.cmd("Neotree float reveal_file=" .. vim.fn.fnameescape(vim.fn.fnamemodify(path, ":p")))
  else
    open_in_float(folder)
  end
end

local function leave(after)
  guard(function()
    report_edits()
    after()
  end)
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

-- A file opened into the diff window (neo-tree opens into the last regular window) moves to the float instead.
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

-- Keeps the float's title on the file it shows, wherever the user navigates inside it.
vim.api.nvim_create_autocmd("BufWinEnter", {
  callback = function(event)
    if float_win and vim.api.nvim_get_current_win() == float_win then
      local config = float_config(event.buf)
      vim.api.nvim_win_set_config(float_win, { title = config.title, title_pos = "center" })
    end
  end,
})

-- However the float closes (ctrl-q or :q), saved edits are reported and the diff catches up.
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

vim.keymap.set("n", "c", comment_on_line, { buffer = diff_buf, desc = "Comment to Claude" })
vim.keymap.set("x", "c", comment_on_selection, { buffer = diff_buf, desc = "Comment to Claude" })
vim.keymap.set("n", "<leader>m", comment_on_line, { desc = "Comment to Claude" })
vim.keymap.set("x", "<leader>m", comment_on_selection, { desc = "Comment to Claude" })
vim.keymap.set({ "n", "i", "x", "s" }, "<C-q>", close_float, { desc = "Back to the diff" })
vim.keymap.set("n", "o", open_file, { buffer = diff_buf, desc = "Open the file over the diff" })
vim.keymap.set("n", "-", open_folder, { buffer = diff_buf, desc = "Open the file's folder over the diff" })
for _, lhs in ipairs({ "q", "<Esc>" }) do
  vim.keymap.set("n", lhs, function() leave(function() vim.cmd("qa!") end) end, { buffer = diff_buf, desc = "Back to the file list" })
end
vim.keymap.set("n", "r", function()
  leave(function()
    local result = vim.system({ "sh", vim.env.REVIEW_SCRIPT, "mark-read", path }):wait()
    if result.code ~= 0 then
      vim.notify("Could not mark read: " .. (result.stderr or ""), vim.log.levels.ERROR)
      return
    end
    vim.cmd("qa!")
  end)
end, { buffer = diff_buf, desc = "Mark read and go back to the list" })
vim.keymap.set("n", "w", function() vim.wo.wrap = not vim.wo.wrap end, { buffer = diff_buf, desc = "Toggle line wrap" })
vim.keymap.set("n", "u", refresh_diff, { buffer = diff_buf, desc = "Reload the diff" })

update_winbar()
