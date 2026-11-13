-- 真实 Neovim TUI 场景（不是 --headless）：
-- nvim -u NONE --cmd "luafile scripts/terminal-capabilities.lua"
-- 使用 Neovim 0.12 的公开 vim.tty.query，保留 UI 供原生观察。
local function run()
  local expected = { Co = '256', RGB = '8/8/8' }
  local counts = {}
  local results = {}
  local names = { 'Co', 'RGB', 'Ms', 'TN', 'sixel', 'iterm2', 'cols', 'lines', 'Glyph', 'unknown' }
  local lines = {
    'XTGETTCAP 真实 Neovim TUI 消费者',
    '版本：' .. tostring(vim.version()),
    'TERM=' .. tostring(vim.env.TERM),
    'TERM_PROGRAM=' .. tostring(vim.env.TERM_PROGRAM),
    '等待公开 vim.tty.query 的 capability callbacks……',
    '注：Neovim 对负答/缺失能力在自身一秒期限后回调 found=false。',
  }
  vim.bo.buftype = 'nofile'
  vim.bo.bufhidden = 'wipe'
  vim.bo.swapfile = false
  vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
  require('vim.tty').query(names, function(cap, found, seq)
    vim.schedule(function()
      counts[cap] = (counts[cap] or 0) + 1
      local wanted = expected[cap]
      local ok = counts[cap] == 1 and found == (wanted ~= nil) and seq == wanted
      results[cap] = (ok and 'PASS ' or 'FAIL ') .. cap .. ': found=' .. tostring(found) .. ', value=' .. tostring(seq)
      local display = vim.list_extend(vim.list_slice(lines, 1, 6), { '' })
      local complete = true
      for _, name in ipairs(names) do
        table.insert(display, results[name] or ('WAIT ' .. name))
        complete = complete and results[name] ~= nil
      end
      if complete then
        table.insert(display, '')
        table.insert(display, '查询完成；按 :q 退出。此场景不宣称 Sixel/Glyph/剪贴板已实现。')
      end
      vim.api.nvim_buf_set_lines(0, 0, -1, false, display)
    end)
  end)
end

if #vim.api.nvim_list_uis() > 0 then
  run()
else
  vim.api.nvim_create_autocmd('UIEnter', { once = true, callback = run })
end
