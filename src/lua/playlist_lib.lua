-- simplified string.find; does a plain search
function string:contains(wat)
   return self:find(wat, 1, true)
end

function string:starts_with(wat)
   return self:sub(1,#wat) == wat
end

function string:ends_with(wat)
   return self:sub(-#wat) == wat
end

function string:set(wat)
   return wat ~= "" and wat ~= "0"
end

function string:unset(wat)
   return wat == "" or wat == "0"
end

metafetch = {any=true}

local metafetch_mt = {}

setmetatable(metafetch, metafetch_mt)

function metafetch_mt.__index(_, k)
   local ret = metadata[k]
   -- no result? empty string
   if ret == nil then return "" end
   -- looks like a number? try returning it as a number
   if ret:match("^[0-9]+$") then
      local as_integer = math.tointeger(ret)
      if tostring(as_integer) == ret then
         return as_integer
      end
   end
   -- okay, just return it
   return ret
end
