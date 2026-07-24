GET {{baseurl}}/info
HTTP 200
[Asserts]
jsonpath "$.std.name" == "webdriver-bidi"
jsonpath "$.std.capabilities" exists
jsonpath "$.std.capabilities['wasi:sockets']" exists
jsonpath "$.std.capabilities['browser:script']" exists
