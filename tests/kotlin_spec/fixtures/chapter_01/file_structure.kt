#!/usr/bin/env kotlin
@file:Suppress("unused")

package sample.feature.ui

import sample.library.Renderer as ViewRenderer
import sample.library.Widget

typealias WidgetName = String

class ScreenModel internal constructor(val name: WidgetName) {
    init {
        require(name.isNotEmpty())
    }

    val widget = Widget()

    fun render(renderer: ViewRenderer) = renderer.draw(widget)

    class Nested
}

object ScreenRegistry

fun topLevel() = Unit

val topLevelValue = 1
