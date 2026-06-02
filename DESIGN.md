# DESIGN.md

## Overview
Brand surface for `grug-brain`: a dark, scroll-led brag doc with archival depth, restrained parallax, and a committed technical palette. The design should feel late-night, local-first, and inspectable.

## Visual Direction
- Color strategy: committed
- Theme: dark, quiet room, large-screen reading
- Mood: calm, confident, technical
- Anti-goals: generic SaaS hero, feature-card grid, neon AI glow, fake app chrome

## Color
- Background: deep charcoal-green neutrals in OKLCH
- Text: pale bone for primary copy, muted sage for supporting copy
- Accent: restrained copper for active lines, tags, and calls to action
- Support: mineral green for graph/history states

## Typography
- Display: `Iowan Old Style`, `Palatino Linotype`, `Book Antiqua`, serif
- UI/body: `Avenir Next`, `Segoe UI`, `Helvetica Neue`, sans-serif
- Mono: `SFMono-Regular`, `Consolas`, `Liberation Mono`, monospace
- Hierarchy: large serif statements paired with compact technical labels and mono metadata

## Layout
- One-page narrative with strong vertical pacing
- Sticky timeline rail on wide screens
- Alternating copy and visual zones using grid, not card stacks
- Hero and transitions use layered absolute elements for depth

## Motion
- Subtle parallax on decorative layers tied to scroll position
- Section reveals through opacity and translation only
- Respect `prefers-reduced-motion: reduce`

## Components
- Thin-outline navigation and call-to-action buttons
- Code/history notes as sparse panels, not repeated cards
- Network/brain motifs via semantic sections plus decorative SVG/CSS layers
