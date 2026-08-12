async def test_restricted_session_denies_script_transitively(
    client, restricted_session_id, with_session, expect_error
):
    # The restricted session is granted navigate + read + input, but NOT script.
    await expect_error(
        client, "evaluate",
        with_session(restricted_session_id, expression="1+1"),
        "std:capability-denied",
        message_contains="browser:script",
    )

    # click transitively needs browser:script to resolve the selector to a
    # node handle, so it is denied even though browser:input IS granted.
    # Asserting the message names browser:script is the point: it proves the
    # coupling documented in the spec rather than just failing on the first
    # missing capability.
    await expect_error(
        client, "click",
        with_session(restricted_session_id, selector="#go"),
        "std:capability-denied",
        message_contains="browser:script",
    )

    # Navigation is granted and still works.
    result = await client.call_tool(
        "navigate", with_session(restricted_session_id, url="https://example.com")
    )
    assert result.structured_content["url"] == "https://example.com"
