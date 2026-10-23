-- Active Neovim layer (TOOLING EDITOR.004): status line + commands.
-- Requires `active` on PATH (or g:active_cli) and active-lsp configured.
local M = {}

local function cli()
  return vim.g.active_cli or "active"
end

local function file()
  local name = vim.api.nvim_buf_get_name(0)
  if name == "" then
    return nil
  end
  return name
end

local function run(args)
  local out = vim.fn.system({ cli(), unpack(args) })
  if vim.v.shell_error ~= 0 then
    return nil, out
  end
  return out
end

--- Status line string: mode, node-budget percentage, taint marker.
function M.status()
  local name = file()
  if not name then
    return ""
  end
  local mode_out = run({ "mode", "--from", name, "--format", "json" })
  local budget_out = run({ "viz", "budget", "--from", name })
  local taint_out = run({ "viz", "taint", "--from", name })
  if not mode_out or not budget_out then
    return "Active ?"
  end
  local ok_mode, mode = pcall(vim.json.decode, mode_out)
  local ok_budget, budget = pcall(vim.json.decode, budget_out)
  if not ok_mode or not ok_budget then
    return "Active ?"
  end
  local used = (budget.usage and budget.usage.nodes) or 0
  local limit = (budget.limits and budget.limits.nodes) or 0
  local pct = limit > 0 and math.floor(used * 100 / limit) or 0
  local taint = ""
  if taint_out then
    local ok_taint, t = pcall(vim.json.decode, taint_out)
    if ok_taint and t.sources and #t.sources > 0 then
      taint = " taint"
    end
  end
  return string.format("Active %s %d%%%s", mode.mode or "?", pct, taint)
end

local function show(title, body)
  vim.notify(title .. "\n" .. body, vim.log.levels.INFO)
end

function M.check()
  local name = file()
  if not name then
    return
  end
  local out, err = run({ "check", name })
  show("Active check", out or err or "")
end

function M.graph()
  local name = file()
  if not name then
    return
  end
  local out, err = run({ "graph", name, "--format", "mermaid" })
  show("Active graph", out or err or "")
end

function M.repair()
  local name = file()
  if not name then
    return
  end
  local out, err = run({ "repair", "--from", name, "--browser" })
  show("Active repair plan", out or err or "")
end

function M.capability()
  local out, err = run({ "capability" })
  show("Active capability", out or err or "")
end

function M.mode()
  local name = file()
  if not name then
    return
  end
  local out, err = run({ "mode", "--from", name })
  show("Active mode", out or err or "")
end

function M.setup()
  vim.api.nvim_create_user_command("ActiveCheck", M.check, {})
  vim.api.nvim_create_user_command("ActiveGraph", M.graph, {})
  vim.api.nvim_create_user_command("ActiveRepair", M.repair, {})
  vim.api.nvim_create_user_command("ActiveCapability", M.capability, {})
  vim.api.nvim_create_user_command("ActiveMode", M.mode, {})
  vim.api.nvim_create_autocmd({ "BufEnter", "BufWritePost" }, {
    pattern = "*.active",
    callback = function()
      pcall(M.status)
    end,
  })
end

return M
