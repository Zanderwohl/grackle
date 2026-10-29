# Toolbar icons

Every toolbar button names an icon, a square SVG at `assets/default/icons/{file}.svg`.
Draw it in white on a transparent background, because the button tints it with its
text color. Avoid text inside the SVG. A missing file draws `hammer.svg`. See
[`src/common/icons.rs`](../src/common/icons.rs).

An icon is named for its picture, not for the tool, so one picture can serve several
tools. Which tool uses which is in code: `Tools::icon` for the map editor, `Tool::icon`
and `Shape::icon` for the prop editor, and the strings in `src/prop/ui.rs` for the rest.
A row in the map's feature list shows the icon of the tool that makes that feature
(`FeatureTrait::tool`).

When an icon ships, check its box.

## Icons

| Done | File | Picture | Used by |
| --- | --- | --- | --- |
| [x] | `hammer` | Claw hammer, head top left, face down, claw up | Construction; the fallback for everything else, including Model and Fit until they have pictures |
| [x] | `cursor` | Classic mouse cursor | Select (map editor), Select (sketch) |
| [x] | `axes` | 3-axis origin arrows | Point |
| [x] | `light_bulb` | Edison-style light bulb | Point Light |
| [x] | `t_pose` | T-pose humanoid figure, legs together | Spawn Point |
| [x] | `mug` | Coffee mug | Prop |
| [x] | `rifle` | Automatic rifle, like an AR-15 | Weapon |
| [x] | `cube_hollow` | Three-rhombus cube, outline only, hollow like the inverting-cube illusion | Room, Box |
| [x] | `cube_solid` | Solid hexagon, the exact footprint of `cube_hollow`, with its edges as negative-space lines | Block |
| [x] | `cube_exclamation` | `cube_hollow` with an exclamation point inside, outlined in negative space to give it a little margin | Trigger |
| [x] | `double_pennant` | Irregular concave pentagon, like the flag of Nepal laid on its long side: two overlapping isosceles triangles of different heights on one base | Footprint |
| [x] | `square_mirrored` | Square on the left, solid vertical line, dashed square on the right | Mirror (map editor), Mirror (prop editor) |
| [x] | `runner` | Running human figure, side-on | Animation Display |
| [x] | `two_runners` | Two running human figures, side-on | Animation Grid |
| [x] | `triangular_prism` | Triangular prism, outline only, lying on one rectangular side, rear lines thinner | Prism |
| [x] | `cone` | Cone resting on its flat base, outline only | Cone |
| [x] | `sphere_shine` | Circle with a specular highlight, outline only | Sphere |
| [x] | `wedge` | Wedge, outline only | Wedge |
| [x] | `square_arrow` | Vertical square, outline only, with an arrow from its center pointing right | Extrude |
| [x] | `axis_arrow_around` | Vertical line with a partial circular arrow turning about it | Revolve |
| [x] | `venn_union` | Circle and square overlapping like a Venn diagram, all three parts filled, lines in negative space | Union |
| [x] | `venn_subtract` | The same Venn diagram, only the square's part outside the circle filled | Subtract |
| [x] | `venn_intersect` | The same Venn diagram, only the overlap filled | Intersect |
| [x] | `two_squares` | Two squares side by side | Row pattern |
| [x] | `squares_in_circle` | Four squares in a circle | Circle pattern |
| [x] | `ground_square` | Flat square on the ground, in isometric view | Plane |
| [x] | `pencil` | Pencil | Sketch |
| [x] | `film_camera` | Film camera with a big reel on top | Frame, Frame selected |
| [x] | `check_mark` | Check mark | Done, Finish sketch |
| [x] | `trash_can` | Trash can, macOS style | Cancel, Delete |
| [x] | `diagonal_line` | Diagonal line from lower left to top right | Line |
| [x] | `eraser` | Square rubber eraser | Erase |
