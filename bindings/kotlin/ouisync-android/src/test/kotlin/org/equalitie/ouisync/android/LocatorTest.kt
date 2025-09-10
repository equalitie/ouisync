package org.equalitie.ouisync.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class LocatorTest {
    @Test
    fun testParse() {
        assertEquals(Locator.ROOT, Locator.parse(null))
        assertEquals(Locator.ROOT, Locator.parse("repos"))
        assertEquals(Locator(repo = "foo", path = ""), Locator.parse("foo/"))
        assertEquals(Locator(repo = "foo", path = "bar"), Locator.parse("foo/bar"))
        assertEquals(Locator(repo = "foo", path = "baz.jpg"), Locator.parse("foo/baz.jpg"))
        assertEquals(Locator(repo = "foo", path = "bar/baz"), Locator.parse("foo/bar/baz"))

        // Document id must be either "repos" or it must contain at least one slash
        assertThrows(IllegalArgumentException::class.java) { Locator.parse("invalid") }
    }

    @Test
    fun testName() {
        assertEquals("", Locator.ROOT.name)
        assertEquals("foo", Locator(repo = "foo", path = "").name)
        assertEquals("bar", Locator(repo = "foo", path = "bar").name)
        assertEquals("baz", Locator(repo = "foo", path = "bar/baz").name)
        assertEquals("qux.jpg", Locator(repo = "foo", path = "bar/baz/qux.jpg").name)
    }

    @Test
    fun testJoin() {
        assertEquals(Locator(repo = "foo", path = ""), Locator.ROOT.join("foo"))
        assertEquals(Locator(repo = "foo", path = "bar"), Locator(repo = "foo", path = "").join("bar"))
        assertEquals(
            Locator(repo = "foo", path = "bar/baz"),
            Locator(repo = "foo", path = "bar").join("baz"),
        )
    }

    @Test
    fun testParent() {
        assertEquals(Locator.ROOT, Locator.ROOT.parent)
        assertEquals(Locator.ROOT, Locator(repo = "foo", path = "").parent)
        assertEquals(Locator(repo = "foo", path = ""), Locator(repo = "foo", path = "bar").parent)
        assertEquals(
            Locator(repo = "foo", path = "bar"),
            Locator(repo = "foo", path = "bar/baz").parent,
        )
    }

    @Test
    fun testIsChildOf() {
        assertFalse(Locator.ROOT.isChildOf(Locator.ROOT))

        assertTrue(Locator(repo = "foo", path = "").isChildOf(Locator.ROOT))
        assertTrue(Locator(repo = "foo", path = "bar").isChildOf(Locator.ROOT))
        assertTrue(Locator(repo = "foo", path = "bar/baz").isChildOf(Locator.ROOT))

        assertTrue(Locator(repo = "foo", path = "bar").isChildOf(Locator(repo = "foo", path = "")))
        assertTrue(
            Locator(repo = "foo", path = "bar/baz").isChildOf(Locator(repo = "foo", path = "bar")),
        )

        assertFalse(Locator(repo = "foo", path = "").isChildOf(Locator(repo = "foo", path = "")))
        assertFalse(Locator(repo = "foo", path = "bar").isChildOf(Locator(repo = "foo", path = "bar")))

        assertFalse(Locator(repo = "foo", path = "").isChildOf(Locator(repo = "bar", path = "")))
        assertFalse(Locator(repo = "foo", path = "baz").isChildOf(Locator(repo = "bar", path = "baz")))

        assertFalse(
            Locator(repo = "foo", path = "bar/baz").isChildOf(Locator(repo = "foo", path = "bar/ba")),
        )
        assertFalse(
            Locator(repo = "foo", path = "bar/baz.jpg")
                .isChildOf(Locator(repo = "foo", path = "bar/baz")),
        )
    }
}
