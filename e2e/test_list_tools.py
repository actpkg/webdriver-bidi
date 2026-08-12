async def test_list_tools_includes_core_tools(client):
    tools = await client.list_tools()
    names = [t.name for t in tools]
    for n in ["navigate", "console_drain", "click", "screenshot"]:
        assert n in names
