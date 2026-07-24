# /tools is POST, not GET, and returns an object with a `tools` array.
POST {{baseurl}}/tools
{}
HTTP 200
[Asserts]
jsonpath "$.tools[*].name" contains "navigate"
jsonpath "$.tools[*].name" contains "console_drain"
jsonpath "$.tools[*].name" contains "click"
jsonpath "$.tools[*].name" contains "screenshot"
