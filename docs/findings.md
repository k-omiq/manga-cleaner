# Findings

What was learned by measuring, testing and building this application. Every number here was
taken rather than estimated, and each carries the conditions it was taken under.

**The machine.** Unless another is named, every figure below is an Apple M5, 10 cores, 32 GB
unified memory, macOS 26.5.1, ONNX Runtime 1.28.0. Nothing here has ever been executed on
Windows or Linux, and a one-machine number is never written as though it were general.

---

## Hardware and acceleration

- **One operator decides where the inpainter runs.** `DFT` is implemented by exactly three ONNX
  Runtime providers: CPU, DirectML and WebGPU. A provider without the kernel does not fail; the
  runtime partitions the graph and copies tensors at every boundary. LaMa's Fourier convolutions
  sit mid-network, so the partition is a dozen round trips rather than a tail.
- **Provider medians, per model.** manga-LaMa 512²: 2540 ms CPU, 3899 ms CoreML, 1311 ms
  WebGPU. Detector 1024²: 703 / 383 / 548 ms. MI-GAN 512²: 415 / 569 / 256 ms. Balloon detector
  640² int8: 129 ms CPU, 222 ms WebGPU, and CoreML cannot build a session at all, taking 8 s to
  say so. Script identification 48×200: 2 ms CPU, 8 ms WebGPU.
- **No single provider wins,** so the provider is chosen per model from one table with a
  measurement beside every entry. The detector belongs on CoreML, both inpainters on WebGPU, the
  small models on the CPU where transfer costs more than compute.
- **LaMa wants exactly two intra-op threads:** 3531 ms at 1 (42% worse), 2486 ms at 2, 2552 at
  4, 2804 at 8, 3848 at 10 (55% worse), 2719 at the runtime's default (9% off). Determinism
  needs one thread, which puts LaMa at 3.5 s, so the golden-test and shipping configurations no
  longer share a latency.
- **Session build is a real cost and is not in the latency table.** CoreML takes 3.6 s for the
  detector's session and 25.7 s for LaMa's, against 0.1 s and 3.3 s on the CPU. A one-page run
  went from 1.6 s to 4.8 s when the detector moved to CoreML.
- **Hold the session.** LaMa's build costs 4.0 s on WebGPU and 3.2 s on the CPU against
  steady-state runs of 1470 ms and 2650 ms. Latency drifts +10.5% on WebGPU and +2.4% on the CPU
  over 40 runs, which is not a leak: a leak would compound rather than settle.
- **Do not batch regions.** The spatial input is fixed at 512², so batch is the only dynamic
  axis: per-region cost is flat from batch 1 to 4 on the GPU, 29% worse at 8, and 2.1× worse at
  batch 4 on the CPU. There is no batching path to build.
- **A forced provider is honoured on speed and gated on memory.** LaMa peaks at 740 MB on WebGPU
  (986 ms) against 8.19 GB on CoreML (1856 ms); MI-GAN at 212 MB (264 ms) against 5.62 GB
  (1311 ms). A forced provider whose measured peak will not fit the room the platform grants is
  declined to the CPU. It gates on a measurement or not at all.
- **Windows gets DirectML for every vendor:** Direct3D 12, so NVIDIA, AMD and Intel come through
  one download, and it has the `DFT` kernel. CUDA is offered and never default: that archive
  carries no DirectML, the process loads exactly one runtime library, and CUDA has no `DFT`, and
  it needs cuDNN 9 installed by hand.
- **The WebGPU plugin gives every desktop platform but one a GPU path.** Because the plugin
  registers into the runtime beside it and carries the `DFT` kernel, Windows has it next to
  DirectML, Linux x64 has it over Vulkan for every vendor, and a CUDA build no longer puts the
  inpainter back on the CPU: the detector goes to CUDA and the inpainter to the plugin. On Linux
  it needs the system Vulkan loader, and a machine without one has the provider registered,
  finding no adapter, and declined for that reason rather than silently absent. Linux aarch64 has
  neither a plugin nor a CUDA archive published and stays on the CPU. AMD's and Intel's own
  providers are not an alternative there: nobody publishes a C archive carrying them, and neither
  ROCm nor OpenVINO has `DFT`, so even downloadable they would take the detector and not the
  inpainter.
- **The Japanese text reader stays on the CPU, and that is a measurement.** One 224 px crop, ten
  runs: encoder 270 ms on the CPU against 218 ms on CoreML, decoder at eight tokens 4 ms against
  15 ms, and the encoder's 52 ms of gain costs a 6.6 s session build. The decoder runs once per
  token, so CoreML loses the part that runs most, and a rescue is a handful of regions on a page,
  so the build never amortises. Unlike the balloon detector, CoreML builds both graphs here and
  is the wrong choice anyway.

## Engines and quality

- **Re-measured through the application's own engines, every level moved.** LaMa at 512², median
  of eleven: 629 ms WebGPU, 1486 ms CPU provider, 1856 ms warm CoreML, session built in 2.1 s,
  against the probe harness's 1311 and 2540 ms on the same machine. The ordering is identical,
  so every decision resting on it stands and none of the numbers in it do.
- **MI-GAN was removed, and the rung it held is left empty.** It was reachable only by escalation
  from manga-LaMa, so every region that arrived there had already been declined by the stronger
  model: a rung whose entire population is what a better engine refused buys nothing, and it cost
  a download and a second held session to buy it. The number 3 stays vacant rather than being
  closed up, because a stored engine ceiling is a name and every comparison is on the rung's
  integer, so sliding the rungs above it down would silently reorder a ceiling somebody had
  already stored. manga-LaMa is the top of the automatic ladder now, and a region it declines is
  left alone and listed for review, which is the ladder's own answer for its end. A patch whose
  stored provenance says `migan` reads as LaMa rather than as an engine this build does not have.
  The measurements in the three bullets below are kept as the record of what was weighed.
- **MI-GAN was fast because it is a 28 MB model.** Warm: 116 to 120 ms on WebGPU (304 ms first
  call), 776 ms CPU provider, 1311 ms CoreML, session built in **51 ms**, forty times under
  LaMa's 2.1 s, which is what lets a live brush open it on demand. A held session is given back
  after five minutes idle rather than at the end of a run, returning 510 MB.
- **The published MI-GAN pipeline was the one that shipped, while it shipped.** It crops, resizes and blends inside
  the model, so the blend was measured: exactly **1 px** outside the mask the graph is handed, against an edit
  margin of 6, over 800 configurations of page, input size, mask size, shape and position. The
  planned re-export would have solved a problem that does not exist.
- **The two model rungs cut deliberately different holes, while there were two.** LaMa gets a stroke-tight hole
  with nothing added; MI-GAN gets the mask grown by a margin, because that graph blends a band of its
  own input back in. Measured at 3 px at 512² and flat across thirty configurations there, 9 px
  at 256², 300 px at 1024²: a property of input size, not of the graph.
- **The stroke-tight hole is a measurement.** Seven geometries were run on each of the eleven
  crowded-fixture regions that reach LaMa: the fitted mask, grown by 8, grown by 16, the convex
  hull, the bounding box, two edge-constrained floods. The stroke-tight hole left the least ink
  on ten of eleven, and the fatter the hole the more ink came back.
- **On a real scan the stroke-tight hole is the wrong hole, which is the opposite of what the
  fixtures said.** Ten regions of one page were rendered three ways, the page before, the first
  pass's hole, and the retry's, which is that set grown by the isolation radius because a re-run
  takes the stored applied mask as its hole. The first pass left stroke-shaped residue in every
  balloon where the retry came back clean: on a scanned page the model reads a stroke-tight hole
  as a glyph's silhouette and draws a glyph into it. The fit grows the ink by that same 5 native
  px past the first step now, walking downward so a balloon whose lettering nearly touches its
  outline gets as much margin as clears the edge map and never a hole across the outline. A retry
  still grows by 5 more. The synthetic table that chose the tight hole stands as what it measured,
  and as the reason a fixture is not a scan.
- **The leftover marks are invented by the model,** which two controls established: a crop
  stretched eight times about the paper level shows no trace of the zeroed text, and a scribble
  control over a crop with every mark painted out moved the written pixels 0 levels on ten of
  the eleven.
- **The decline metric does not fire on the failure this corpus contains.** Where LaMa invents
  text into a balloon, the interior-to-surround edge-energy ratio is 0.15 to 0.37 against a
  threshold of 2×, and all seven scored outputs pass: a balloon's annulus is flat paper crossed
  by its own outline, so its median is 0 and its mean is the outline.
- **The optional local redraw rung, measured.** FLUX.2 Klein 4B, Apache-2.0, MLX 4-bit,
  4,619,705,407 bytes on disk. One 512² render at 4 steps peaks at **6.58 GB** of physical
  footprint and settles at **1.37 GB** after release; 1024² peaks at **11.46 GB**. Opening costs
  0.6 s, rendering about **21 s** at 512² and **161 s** at 1024². Unguarded, the same 512²
  render costs **15.36 GB**, so its memory guards are worth **2.3×**.
- **Two of the four guards were withdrawn once the rung met real fixture text.** Evicting the
  text encoder made the second render through one open throw, and tiled decoding seams the
  resolution small crops are upscaled into. Keeping the encoder costs 8.92 GB across two renders
  (19.5 s and 26.6 s for a 288×608 and a 192×288 crop) and 9.77 GB at the largest untiled shape
  of 768², so the declared working set rose to 10 GiB.
- **A second backend, measured on Metal only.** With 4-bit SDNQ under `torch` and `diffusers`:
  5.48 GB of weights, open 4.5 s, renders of 47.8 s and 38.4 s, 7.00 GB peak across both.
  Roughly twice the wall clock and 1.9 GB cheaper at the peak, so the backend is a setting
  rather than a platform fact.
- **A third backend was measured and not adopted.** `stable-diffusion.cpp` at 768² and 4 steps
  with 5.49 GB of GGUF weights: 78.9, 83.5 and 94.8 s at 6.63 to 6.79 GB peak, so 3.1× to 4.9×
  slower than MLX. Offloading to the host was worse on unified memory (107.5 s, 8.71 GB), and
  its video-memory budget was a no-op, merging 29 planner segments into 1 with the footprint
  unmoved. Output was clean but lifted background tone from 246 to 253.
- **Sizing the redraw crop to the region was that rung's largest quality fix.** Grown by 64 px
  of context and then floored at 512², a 40×140 balloon region came back as halftone smudged
  over the text; the same region in a crop sized to itself came back clean. Context is now half
  the long side clamped to 24 to 80 px, snapped up to a multiple of 16, with no floor.
- **Sixty-six renders across eight axes, one parameter changed at a time.** 768 is the working
  resolution: 512 reproduces the smudge, and 1024 and a one-megapixel area target both paint
  invented texture over a flat white balloon for 2× to 6× the clock. Guidance stays 1.0 and
  steps stay 4. The prompt is now one colourspace-agnostic instruction, replacing a pair whose
  screentone arm was clean at seed 1 and no other seed tried.
- **The hardware gate's arithmetic is unified-memory arithmetic, not total RAM.** macOS caps
  Metal residency at roughly 75% of physical memory, 21 to 24 GB of a 32 GB machine. Worked:
  physical 32,768 MB, room 21,665 MB, resident cleaner 2,510 MB, admitted with 20,085,822,804
  bytes. An 8 GB Mac is left 2.77 GB, below the weights alone, and is refused.
- **The cloud rung's costs and terms.** Paid tier only, enforced, because the unpaid tier trains
  on submissions with human review. No mask parameter exists, so a mask sent as a reference
  image is advisory, and no per-ratio pixel table is published, so sizes are probed once per
  model, ratio and tier, about 40 calls under $3. Per image: $0.045 at 512px, $0.067 at 1K,
  $0.101 at 2K, $0.151 at 4K on the flash model, $0.134 at 1K and 2K and $0.24 at 4K on the pro
  model, halved in batch. Its watermark is mandatory and robust to crop, therefore spatially
  distributed, so every returned pixel is potentially altered.
- **The drift gate's primary test is how much correction the model needed.** A uniform tone
  shift is exactly what an affine fit absorbs and it reproduces on an adjacent held-out half, so
  a fit needing a bias beyond 8 levels or a gain outside 0.9 to 1.1 is rejected and clamp
  saturation is a rejection cause. The ring must be at least 32 px wide: at about 10 px the
  outer half gives one block row, so a p99 over roughly 50 samples is the maximum, the statistic
  it claimed to replace.

## Detection and the script gate

- **The detector's output contract, pinned.** 94,669,756 bytes; `images float32[1,3,1024,1024]`
  in; `blk [1,64512,7]`, `seg [1,1,1024,1024]`, `det [1,2,1024,1024]` out. Bind by channel
  count, never by index: upstream carries a defensive swap for exports that reverse two outputs,
  and only the channel count tells them apart.
- **The box classes are languages, not bubble kinds.** The model emits English and Japanese,
  over which upstream itself comments that the class may be wrong. The bubble, text-bubble and
  free-text taxonomy the script gate was built on belongs to a different detector, discussed
  four documents away as the permissive alternative. The scoping survived; the mechanism did not.
- **Detection resolution is a fixed 1024² letterbox,** so the scale factor is the long edge over
  1024 and a 1600×2400 page is scaled by about 2.34. A height-band pre-scale described as
  leaving that page untouched belongs to another tool and sits in front of a detector that
  resizes again: not merely inert, lossy twice. That is also why long pages are cut into
  segments first: an 800×12000 page letterboxed whole is a scale factor of about 11.7.
- **The balloon detector is cheap and accurate:** 11.1 MB int8, Apache-2.0, RT-DETR-v2 rather
  than a YOLOv8 derivative, **0.25 s a page**, emitting exactly the three classes the gate
  wanted, with boxes within about 10 px of the fixture's drawn ellipses.
- **Its size input is width-first,** against the documented height-first order. With the
  documented order every box returns with x multiplied by H/W and y by W/H, 1.5 and 0.667 on a
  1600×2400 page: confident detections in the wrong places, which reads as a bad model rather
  than as a swapped pair.
- **The balloon-geometry second opinion was decided by ring shape.** A closed rectangular ring
  inside a rounded balloon leaves it at the corners long before the sides run out, reads the art
  beyond the rim, and calls picture on a region the detector had right. Sampling the middle half
  of each of four sides turned **43 of 80** swept geometries from textured or unreadable into
  solid, with open screentone still textured.
- **The script model's input convention is undocumented and cost real time.** Input is
  `float32[1,1,48,W]` and is one text line, not a block. A vertical line is rotated
  counter-clockwise, and a mirrored rotation returns plausible labels with no error and no low
  score. A crop whose median is under 64 is inverted first, and values use the source
  recogniser's black and white normalisation. Its output is CTC and the class labelled `Broken`
  is the blank, so averaging the score field returns `Broken` for every input ever tried.
- **Two conventions on top of that.** A recognition margin of 15% of the line's cross-axis size,
  because a hiragana column reads as vertical Japanese at 60 px wide and as Latin at the 43 px
  its mask occupies. And `Common`, `Joined`, `NULL` and `Broken` are abstentions rather than
  votes, since short columns return `Common` and counting it against Japanese would fail the
  punctuation-only boxes that must be classified Japanese.
- **Every CJK label is treated as one class.** The documented confusion mass is Chinese against
  Japanese and it showed up here, a hiragana column returning simplified Han. The question
  actually asked is whether the text is Latin typesetting a localiser added, and against that
  every CJK label is the same answer. Hangul is excluded.
- **Sound effects are conceded, not classified.** The best published detectors reach 61.2%
  H-mean on onomatopoeia boxes and 67.8% on polygons, against 0.889 to 0.918 average precision
  for bubble text in the same corpus, a gap of about 30 points. Their named failure modes break
  any classifier that could be built here.
- **Size thresholds scale by text size, not page size.** Absolute cutoffs are meaningless across
  tankoubon and webtoon, and the fraction-of-page-area replacement was worse: 8e-3 of page area
  is 7,680 px² on an 800×1200 webtoon, 31× smaller than the 240,000 px² dialogue box used to
  condemn the absolute version. Thresholds are relative to the page's median box area.
- **The small-box drop was throwing away dialogue the detector was sure of.** The rule is aimed at
  speckle, and it read the box's size and nothing else. On twelve pages of a real scan, seven
  boxes the detector was 0.84 to 0.94 sure of fell under 0.15 of the page's median box area and
  left the run entirely, not into review but off the page, each of them a short single column of
  dialogue inside a balloon the balloon detector scored 0.74 to 0.90. The one small box on those
  pages that really was noise scored 0.41. So the drop no longer applies at or above a confidence
  of 0.7, the middle of that gap, and the exemption is the drop's alone: an oversized box is still
  flagged whatever the score, because flagging costs nothing and dropping could not be undone.
- **The paper tolerance has to come from the paper, not from a constant or from the page.** On six
  pages of a real scan 32 of 56 regions read as out of a balloon, and every one of the nine the
  balloon detector had placed inside a bubble at 0.88 to 0.94 is a white speech bubble in the
  crop. The scan's paper has a spread of 7 to 10 levels against a fixed tolerance of six, so a
  fifth of every ring was off the fill. The tolerance is now the innermost rings' own spread, the
  92nd percentile of deviation times 1.5, floored at 6 and capped at 20 levels; the page's
  flattest-decile noise could not stand in for it, because it measures 0 on three of the six pages
  whose flattest tiles are saturated white. After the change, 20 of 56, all of them regions the
  detector itself labels free text.
- **A balloon drawn as two lobes was reported as text outside a speech bubble.** The detector emits
  one bubble box per lobe and no text-in-bubble box at all; the lettering nearly fills the shape,
  so the ring walk meets the outline on its first ring and reads picture, and ordinary dialogue
  went to review. Six regions across eighteen probe pages. Sure bubble boxes now overrule the
  paper reading when they cover 75 per cent of the region and one of them holds its centre, and
  coverage is taken over the **union**, which is the load-bearing part: no single lobe covers even
  a third of such a region. The separation is wide, 0.79, 0.87, 0.93 and 0.97 against 0.00 and
  0.34 for the two regions that must stay outside, which fail the centre test as well. The union
  is one balloon's lobes and not every sure shape on the page: a box joins it only by touching or
  overlapping a box that holds the centre, which stops a neighbouring balloon lending the missing
  quarter. A free-text box still wins outright wherever the model drew one.
- **Text the text detector never boxed had no region at all, and the balloon detector had already
  seen it.** Inside a balloon the text detector is complete: over 28 real scans there is a
  text-detector box under every text-in-bubble box. Outside one it is not, and the miss is silent,
  because a region that is never built is not cleaned, not flagged and not listed. Nine boxes
  scored 0.53 to 0.88 had no region over them at all, narration boxes, a caption line running the
  page's width, a stylised sound effect, a chapter title strip and a credits line, and two more
  came out with them for eleven adopted regions. The balloon detector's text boxes are a second
  source of regions now: a sure box that nothing already covers becomes a region and is grown
  through the same four tiers as a detector box. Covered is the page's existing pair of tests, the
  box's centre in some region's masking rectangle or an overlap above the share the extended tier
  already merges on, because centre alone adopts a wide caption whose middle falls between two
  boxes of that same caption and overlap alone adopts a small box in a large region's corner. The
  size rules apply by half: the large-box flag applies, the small-box drop does not, since such a
  box exists precisely because the text detector emitted nothing there to measure it against. The
  joined list is sorted by position before anything counts it, because a region's id is its index.
  The seed was measured before any of it was built: 1,204 px of segmentation under the smallest
  box up to 21,040 under the title strip, a quarter to a third of the box on the large ones. The
  two heads of the detector disagree, and the segmentation head marks the ink the box head never
  emitted a box around, which is what makes the adoption cheap.
- **A narration box has no room around it, so the ring walk read it as picture.** The walk answers
  the paper question only where there is room for an answer; a rectangular narration box is drawn
  with its frame tight against its lettering, so the first ring is already on the frame and beyond
  the frame is the art the plate sits on. Four of them went to review as text outside a balloon
  while the same pages' balloons cleaned. The walk is the first opinion now rather than the only
  one: where it finds no band, the paper **between** the strokes is read instead, every on-page
  pixel the grown text mask does not claim, measured by the same machinery the rings are. The
  order is one way on purpose, since a walk that found a real band has measured a real balloon and
  the inside can add nothing to it. Two thresholds separate the cases and both were measured.
  One-sidedness refuses art, which strays both lighter and darker than its own median, while ink
  strays one way only; that alone is not enough, because a screentone is one-sided too, so the
  off-fill allowance is twice a ring's, a ring being a line of clearance where anything off the
  fill is a defect and the inside of a text box containing ink by construction. Over the 28 scans
  the four narration boxes sit at 6, 10, 10 and 14 per cent of their samples against 19 for the
  nearest thing that must not read as paper, a gap running from 15 to 17 with twice the ring's
  eighth landing in it. The paper floor was measured the same way: the narration boxes keep 50 to
  62 per cent of their pixels after the halo and the least any of the 254 regions keeps is 15, so
  a fifth is a floor against a box that is all stroke rather than a threshold anything real sits
  near. **Twenty of the 254 regions changed verdict, every one of them from out-of-balloon to
  clean or uncertain and none the other way.** What did not move is what the reading is bounded to
  exclude, each checked from its crop: a sound effect over art at 70 per cent off the fill and
  two-sided, a caption over tone at 79, and a credits line reversed out of dark artwork at 74,
  which was expected to flip and does not, because that line is not on paper.
- **A Japanese-only reader cannot be the gate and is the right instrument for the gate's
  failures.** The identifier returns uncertain on ordinary dialogue in ordinary balloons, and
  occasionally names something exotic, Tibetan on a tall kana column, Syriac on another, because
  those scripts are also tall and thin and a 48-pixel strip of vertical kana is inside their basin.
  Over the 28 reference scans, 254 regions, **12 were offered to the reader and 12 rescued, 4.7
  per cent, with none offered and declined**, and every one is a speech balloon a person would
  clean without hesitating. The rescue runs under three conditions, each a refusal to widen: the
  balloon detector placed the region confidently inside a bubble, since a reader whose whole
  vocabulary is Japanese will read something off a sound effect too, so the permission cannot come
  from the reader; the identifier failed, either uncertain or naming a script outside a short
  trusted allow-list, so a confident Latin is never second-guessed; and the reading is at least
  two characters and at least 60 per cent written in a Japanese block, on the same letters-only
  rule the gate's own decision rules use. A rescued region carries a script of its own, so
  provenance and the probe tell a rescue from an identification without re-running anything. The
  negative control is the page with the title strip and the credits line: nothing on it was offered
  to the reader at all, because neither of those is inside a confident balloon and the balloons
  were already clean from the identifier. The reading is evidence about script and not a
  transcription anyone consumes: one rescue is very likely wrong about the characters and is still
  right that the balloon holds Japanese, which is the only question asked. It costs 164 ms for a
  ten-character balloon on the CPU, 232 at 26 characters, 275 at 28 and 808 at 36, decoding
  greedily and stopping at 64 tokens.
- **Fixtures are generated and are not a corpus.** The detector emits no box for either large
  sound effect on one of them while the segmentation mask has clear signal inside both, the
  conceded weakness appearing where predicted. And three of the four have a noise floor of 0.0,
  so every region routes to flat fill and the denoise rung never executes: the page added for it
  carries normal grain at sigma 2.6 levels, because a uniform draw over a handful of levels is
  bimodal by the ring's own test and would be refused as multimodal.

## Mask fitting

- **Growing the mask natively removed two corrections rather than compensating for them.**
  Growth on the proxy needs a native annulus, because the upscaled contour lands on the
  anti-aliasing fringe and biases the median 5 to 20 levels dark, and needs the periodicity check
  on a two-dimensional patch, because a nearest-neighbour stair-step is itself periodic at the
  scale factor. It is free, and at proxy resolution the gate's line splitting flipped three of
  five regions from vertical to horizontal and turned three Japanese verdicts into uncertain ones.
- **The strong-edge rule was carried for the mask and not for the ring one pixel outside it.** A
  bubble stroke inside the annulus is tens of levels of deviation on flat paper and self-similar
  along its length, so it fails the deviation test and triggers the periodicity test at once.
  Four regions on the crowded fixture were routed to the inpaint ladder by a statistic that had
  measured the balloon rather than the paper.
- **No threshold could have separated the two.** A stroke's peak autocorrelation is 0.91 against
  0.58 to 0.81 for real halftone fields, so the stroke scores higher than the thing the test
  looks for. Only the pixel set could fix it: the ring is restricted to the paper on the mask's
  own side of the page's strong edges.
- **Screentone is not far below the strong-edge threshold.** On the crowded fixture 24 to 29% of
  screentone pixels are strong: dots at level 60 on paper 246 is 0.82 of the page's peak gradient
  against a threshold fraction of 0.5. What keeps a mask off screentone is that its border lands
  on a dot, so the same test fires for the opposite reason to the documented one. The test named
  for the claim built its tone at 200 on paper 250, a fifth of the real contrast.
- **Periodicity is autocorrelation over the annulus only:** over its bounding box every
  flat-paper balloon on the dialogue fixture came back periodic, since a column of glyphs is
  self-similar at its own pitch. Edge crossing is gradient magnitude on the border only, since
  testing the interior rejects the first candidate on every page.
- **The fail threshold is relative to the page's noise floor.** A fixed 8.0 sits below the border
  deviation of flat white on a JPEG raw at quality 75, which is 5 to 12 from blocking alone. It
  is the larger of 8.0 and 2.5× the page noise sigma, measured once per page from its flattest
  decile, and the denoise trigger is relative for the same reason.
- **A region carries two masks, and treating them as one put a visible box on the page.** On
  flat paper every candidate's deviation is zero, so selection ratchets to the end of the series
  and the mask is the seed grown by 4 + 11 × 2 = 26 proxy pixels, 64 native at a 2.34 scale: a
  65×169 detection box became a 38,985 px mask, 355% of the box's own area.
- **That growth is free for a fill and ruinous for a model.** LaMa answers a hole a level or two
  off the page's tone, measured at 2.5 8-bit levels of drift even 65 px from the hole, and a
  mask grown across a balloon gives that offset a long smooth border to show itself along. One
  region: 33,912 pixels written and a visible blob, against 12,467 and none through the narrower
  mask. The model's input is unchanged either way.
- **A region is never dropped for want of an admissible mask.** When the first growth step's
  border sits on a strong edge, no member of the series is admissible, but the radii below it
  are not members and each is tested in turn. On seven such regions this also cut residual ink:
  337, 406 and 8,311 pixels against 642, 627 and 11,666.
- **The denoiser is bilateral over a 5×5 window with its range sigma set to the page's noise
  floor,** the same measurement its trigger uses, which lets one rung serve a clean scan and a
  JPEG raw with no tuned constant between them. Twenty-five samples cut a deviation by up to 5×,
  while a wider window costs quadratically and starts averaging the structure it must preserve.
- **A 5 px dilation composed with a radius-1 feather escapes a 6 px edit margin.** The element
  is a disc at radius 5 and a square at radius 1, so the composition reaches the square root of
  37, about 6.08 px, at four corners per region. One dilation by 6 does not. Off by 0.08 px on
  four pixels, found by a test asserting the bound rather than by inspection.

## Long strips and memory

- **The measured budget, process RSS including the webview.** Webview 120 to 180 MB; core idle
  about 40 MB; detector session about 195 MB against a design figure of 50; **detector allocator
  arena about 1.2 GB**; script model about 10 MB; MI-GAN session about 20 MB against a design
  figure of 90; LaMa session about 350 MB on WebGPU and 410 MB on the CPU provider; decode
  window and working buffers about 30 MB.
- **The arenas invert by provider at roughly 30×.** MI-GAN's is about 15 MB on WebGPU and 345 MB
  on the CPU provider, LaMa's about 10 MB and 310 MB, because on WebGPU the activations live in
  GPU buffers that never enter the resident set. Neither number is predicted by anything on disk.
- **Whole-run peaks, eight copies of a twenty-region page through one held session:** 2.07 GB at
  flat fill and denoise, 2.45 GB with LaMa, 2.24 GB with MI-GAN. Independent of document length
  and of region count.
- **Length independence holds; the level was four times the estimate.** Over 200 synthetic
  800×1280 pages at the two cheap rungs: 5 MB before any model, 327 MB with three held sessions,
  1506 MB after the first detection at 1024², 1607 to 1707 MB for pages 3 to 200, peak 1.89 GB.
  The peak arrives inside two pages and does not move over the remaining 198.
- **The arena is what a budget written as a list of resident objects has no row for, and it
  steps once per distinct input shape.** One inference over the fixed 1024² input adds about
  1.2 GB and never returns it; over eight copies of one page it rises 1.2 GB on the first
  detection and 400 MB more on the second page, the first segment crop of a different height,
  then does not move. A step, not a slope.
- **A held LaMa session is two objects.** A reading of about 510 MB is the digest's read of the
  weight file, which leaves 198 MB of a 207 MB file resident and never returns it, plus 347 MB
  for the session. The pressure ladder's unload step therefore returns 350 MB, not 510.
- **The obvious pressure signal on macOS reads 0.** The per-process available-memory call
  reports room under a per-process limit, and a plain desktop process has none, so read as bytes
  available it steps every machine all the way down at the first reading and the step latches. A
  constant signal is not conservative; it is a disabled feature that looks like a working one,
  and it was linked and called rather than read about because the failure is silent.
- **The signal used is the kernel's own memory pressure level,** polled between regions, the
  only moment at which what is resident is the sessions rather than a region's buffers. Under
  pressure the order is largest and most optional first: refuse and unload the redraw sidecar,
  then unload LaMa, then drop to one window in flight (one under 16 GB of machine memory, two
  above). Windows and Linux have no equivalent signal and take no step.
- **Resident set size cannot see unified memory, and that error survived a whole revision.** Six
  instruments on one guarded 512² render: three resident readings agree at 2.883 GB and three
  footprint readings agree at 6.576 GB, so there is nothing to average. A mid-render sample says
  where it is: graphics accelerator 3,842 MB, large allocations 153 MB, small allocations
  140 MB, untagged 133 MB, and 46 MB of owned footprint that is unmapped and that no resident
  reading can ever contain.
- **The instrument does not merely understate, it does not move.** At 1024² the true peak is
  11.46 GB and the resident reading is 2.90 GB, eighteen megabytes above what it read at 512²
  while the real cost grew by 4.9 GB. A peak below the 4.6 GB weight size should have wanted an
  explanation, and the runtime self-reported 4.90 GB in the same run.
- **Split planning.** Splits target 2000 px with a tolerance of 500 widening to 1000 and 1500,
  triggered below a strip aspect ratio of 0.33, and a candidate row is accepted only below 10%
  of the page median. Rows are scored by edge energy, not ink density, which has the wrong sign
  on inverted content: white text on a black panel is a low-ink row. The profile is one float
  per strip row, so a 200-page webtoon is 3.2 MB of profile and one page of pixels.
- **The interface's chapter-scale allocation was region state, not pixels.** Opening a chapter
  answered with every page's region list, four thousand objects for a 200-page chapter, live for
  the session. Three pages' regions are resident now, plus per-page counts and a review index at
  about 90 bytes per flagged region. Undo history was the other half: a command held as a
  closure over two region snapshots pinned every page the user had edited.

## Colour fidelity and export

- **The canvas-based export path corrupts every file it touches, before any engine runs.**
  Canvas has one pixel format, 8-bit sRGB with alpha, so it promotes grayscale raws to RGB,
  truncates 16-bit to 8, discards the ICC profile, cannot represent CMYK or indexed sources,
  destroys source alpha by filling white first, and re-encodes untouched pages. The intermediate
  is corrupted the same way, so it is not salvageable by patching.
- **Twelve fixtures round-trip byte-identically** through the pure-Rust codecs: gray and
  gray-alpha at 8 and 16 bits, RGB and RGBA at 8, RGB at 16, three ICC variants, indexed with a
  free palette slot, bitonal, and CMYK TIFF. Bitonal was added because a 1-bit image is the only
  one whose rows pad, the case a length assumption gets wrong.
- **Writing the PSD format was cheaper than hardening a crate that writes it.** What this
  application asks of the format is small and fixed: a header, an ICC resource in image resource
  1039, a background layer, one masked layer per region in one group, and a merged image. With raw
  channel data, which every reader accepts, every length in the file is arithmetic on the
  dimensions, nothing is buffered in order to be measured, and there is no run-length writer to
  drop a byte. That is a few hundred lines, and in those lines grayscale, alpha, CMYK and 16-bit
  cost the same as 8-bit RGB: a channel count, a mode code, a bytes-per-sample. Vendoring would
  have bought a hardened 8-bit RGB path plus a grayscale patch and left 16-bit deferred. Two of
  the format's less obvious rules are followed the way Photoshop follows them, because a reader
  that is not Photoshop would not have caught either: a 16-bit document keeps its layer records
  under the `Lr16` additional-info block with the standard layer info empty, and CMYK is stored
  inverted. An independent reader opens every fixture the writer produces, layered and flat, at 8
  and 16 bits, with the mode, depth, layer tree, layer masks and merged pixels expected. The files
  are larger than Photoshop's own, roughly the uncompressed page per layer, which is the price of
  the property that makes every length computable.
- **The mask file is made of the ink, not of what an engine painted.** The union of each patch's
  ink mask, the lettering the edit removed, rather than the union of the applied masks: on a flat
  fill the applied mask is the whole grown balloon, and a file made of those is a page of white
  blobs where the balloons were, which is not what a typesetter asking where the text was wants. A
  layer's own mask inside a PSD stays the applied one, because a layer mask states what the layer
  contributes.
- **Pages mostly do not arrive in a format this stack writes, so they are converted once, at
  ingest, into the library.** A JPEG, WebP, GIF or BMP is decoded by a pure-Rust codec and written
  as a lossless PNG, a CMYK source as TIFF, into the job's own folder, and a PNG or TIFF takes the
  same road with a byte-for-byte copy in place of the decode. That file is the page from then on,
  so everything downstream of ingest sees PNG and TIFF and nothing else, and the user's folder is
  read once and never read or written again. Four codecs is where the list stops because all four
  are pure Rust: AVIF wants `dav1d`, HEIC wants `libheif` and JPEG XL wants `libjxl`, and taking
  any of them reopens the decision that removed the C image library. Two consequences are stated
  rather than absorbed: a chapter's disk cost is now a second copy of every page, on the order of
  a gigabyte for a 200-page colour chapter with no free-space check in front of it, and the resume
  hash is over the job's own copy, so a user re-cropping the scan they ingested from no longer
  marks anything stale. What the hash still catches is the failure it was written for, now over a
  file this application owns: truncation, corruption, or the page going missing under a job about
  to resume onto it.
- **The sRGB and ICC chunk trap is worse than "never set both".** The encoder writes the profile
  in the else branch of a test on the sRGB intent, so a source PNG carrying both chunks, which
  the specification allows, silently loses its profile on any re-encode that copies the decoded
  header across. The intent is kept out of the header and the sRGB chunk written by hand.
- **Indexed sources get flat fill alone.** A denoiser averages levels and a palette index is not
  a level: the mean of index 3 and 5 is index 4, an unrelated colour chosen by whatever order
  the palette happens to be in. Mapping back to the nearest entry is per-pixel near-white
  snapping, and inpainted output is continuous-tone and unrepresentable in a palette at all.
- **CMYK is restricted to the two exact rungs.** The model rungs are RGB-only and the round trip
  is not invertible: rich black at C60 K100 becomes RGB black and returns as K100, so an
  inpainted patch carries different ink separations from its neighbours. Invisible on screen,
  ruinous in print, which is the only reason CMYK is supported.
- **A silent substitution is a different failure from a downgrade.** A format the build could
  not produce was mapped to "same as source", so a user who chose JPEG received PNG and was told
  the export succeeded. That never violated the pixel contract, since the substitute was always
  lossless. It violated the word "silently", and only one of the two is visible in a test.
- **Passthrough is decided by per-page intersection with the global patch set, not by patch
  ownership,** which is the answer already sitting in the manifest. Take ownership and the page
  below a join reports zero applied patches, is copied byte for byte, and arrives with the
  bottom of a bubble missing, while its file is bit-identical to its source and so passes the
  fidelity test, which is an upper bound on change and cannot see a change that did not happen.
- **The subset property holds by construction, which is exactly its limit.** The compositor only
  writes inside the mask plus isolation, so no model response can break it, and the test
  therefore says nothing about whether the set written was the right size. Reading it as though
  it did is what put a visible box on a page.
- **Model output is not bit-reproducible, and not only across providers.** The same provider and
  version can differ between consecutive runs, with a documented scatter-operator case cured
  only by a single intra-op thread; machines differ through instruction-set dispatch; the graph
  optimisation level changes outputs. Golden tests are therefore tolerance-based at about 1e-4
  on the CPU provider only, and everything upstream of the models is made deterministic instead.

## Runtime and packaging

- **Fetching the runtime after install costs one entitlement.** The signing matrix was run five
  ways: an ad-hoc process loads an ad-hoc runtime; a Team ID process under the hardened runtime
  refuses it, the system reporting different Team IDs; with library validation disabled it
  loads; a runtime re-signed with the same Team ID loads with no entitlement, which is only
  available for a library that ships in the bundle. Every official release is ad-hoc and
  linker-signed with no Team Identifier, so the design requires disabling library validation,
  which weakens the process for every dylib and is recorded as a trade, not a free sidestep.
- **The runtime is not one build, and providers differ per archive:** macOS arm64 at 31 MB with
  CPU, CoreML and WebGPU; Windows x64 and arm64 at 76 and 77 MB, CPU only; Linux x64 and
  aarch64 at 9 and 8 MB, CPU only; the DirectML package at 13 MB plus its 202 MB dependency;
  the two Windows CUDA archives at 449 and 353 MB with CPU, CUDA and TensorRT; and no Intel
  macOS archive published at all.
- **The provider name strings are in every build and prove nothing.** A check that grepped them
  reported that all six archives carried every provider, which is how it was found to be
  worthless. What ships only where a provider was built is the provider factory headers and the
  provider libraries. The script had nonetheless drifted back to the discredited method, so the
  artefact meant to re-derive the table could not have produced it.
- **Three facts fell out of reading them properly.** The CUDA archives carry no DirectML, which
  makes the two mutually exclusive, and no CUDA runtime or cuDNN, which makes CUDA the only
  provider here with an install behind it. From 1.28 the native WebGPU provider ships as a
  separate plugin, so the macOS archive is the one that still has it compiled in. And the
  Windows GPU download is 215 MB and not 12, because that path is two artefacts and the second
  is the large one, which had been the wrong number in an argument used against CUDA.
- **The separate WebGPU plugin is one artefact for four platforms, and it is taken.** 39 MB, one
  package covering Windows x64 and arm64, Linux x64 and macOS arm64, and it registers into any
  runtime from 1.24.4 up, which is every build in the table. It therefore ships as a companion
  download beside every Windows and Linux x64 runtime, is registered once at load and reported as
  absent, built in, registered or failed rather than failing the load, and is skipped where the
  runtime already has the provider compiled in, which is macOS. The library API for plugin
  providers already existed, so nothing was hand-rolled through the C API. On Windows the
  runtime's own directory is put on the library search path before the plugin is opened, because
  the plugin is loaded by name and pulls in two shader libraries by name, and neither searches the
  directory it came from. Linux gained two NVIDIA archives on the same terms as Windows, 424 and
  241 MB, whose digests came from the release's asset record without downloading them.
- **Pixels reach the webview over a custom protocol, and Windows is the constraint.** Base64
  data URLs inflate payloads by 33% and decode on the main thread; one reported migration of a
  150 MB response went from about 50 s to under 60 ms. The Windows webview raises its
  resource-request event on the host UI thread and pauses page load: a maintainer benchmark
  measured about 5 ms on macOS against about 200 ms on Windows for 10 MB, roughly 50 MB/s, with
  no range or streaming responses, so responses must be whole tiles.
- **The Windows runtime imports the Visual C++ runtime outright, and nothing shipped it.**
  Parsing the PE import directory of the shipped
  `runtimes/packages/.dml/runtimes/win-x64/native/onnxruntime.dll`, its regular imports are
  `MSVCP140.dll`, `MSVCP140_1.dll`, `VCRUNTIME140.dll` and `VCRUNTIME140_1.dll` beside
  `KERNEL32`, `ADVAPI32`, `SETUPAPI`, `dbghelp` and ten `api-ms-win-crt-*` names. Regular and not
  delay-loaded, so they resolve when the library is mapped and their absence fails the load
  outright. The ten `api-ms-win-crt-*` names are the universal CRT and are part of Windows 10, so
  only the four `140` names are a user's problem. They are in neither NuGet package, the
  installer bundled WebView2 and not them, and a machine without the redistributable could
  therefore do nothing at all: `LoadLibraryExW` fails with 126 and the interface called it
  `unloadable`, a sentence with no remedy in it. Fixed twice over - the libraries are staged
  beside the executable from the build host's own Visual Studio redist folder, and the loader
  reports a missing dependency as its own case so that a machine the staging missed is told what
  to install.
- **App-local is the right place for them because of how `ort` opens the runtime.** It calls
  `libloading::Library::new`, which is `LoadLibraryExW(path, NULL, 0)` with no
  `LOAD_LIBRARY_SEARCH_*` flags, so the runtime's own imports resolve through the standard search
  order, whose first entry is the directory the application loaded from. Tauri puts bundled
  resources in that same directory on Windows, verified by running `tauri_utils`'s own
  `ResourcePaths` rather than read out of the documentation. The trade is real and is not free:
  an app-local copy gets no Windows Update servicing, so a CRT security fix ships as a release of
  this application.
- **Two operational traps around the runtime download.** A quarantined copy is refused outright
  and loads once the attribute is cleared, which a browser download sets and a library download
  does not; and the dylib's own load command names a versioned filename, so a copy saved under
  another name fails to resolve itself with an error that reads exactly like a signing failure.
- **Throughput has a target because nothing else measured it:** at least 20 pages per minute on
  the two cheap rungs. Detection at 350 to 383 ms a page on CoreML leaves that budget intact.

## Models and licensing

- **Mirrors re-tag checkpoints under licences their sources did not carry.** Three separate
  mirrors here did it: one detector export is tagged Apache-2.0 while the same author's weights
  repository and the upstream project are both GPL-3.0, and one inpainting export is tagged MIT
  while its source checkpoint comes from a GPL-3.0 project with undisclosed training data. Trace
  weights to their training source. The same standard applies to bug reports: a claim that a
  defect was fixed upstream is a claim about a version, and the version pinned here still had it.
- **Every YOLOv8-derived detector is AGPL-3.0 through its training framework,** regardless of
  the repository's tag, and the framework's position is that this holds even for models trained
  from scratch. Two Apache-tagged candidates were voided by it. Check architecture lineage, not
  the model card.
- **Verified clean.** The LaMa architecture upstream is genuinely Apache-2.0 with no
  non-commercial clause, and the manga finetune declares MIT at 204,544,673 bytes with a
  published ONNX export at 207 MB and opset 17, so the default inpainter was never
  licence-blocked and the revision that demoted it had no licensing basis. The script model is
  Apache-2.0 at 3.72 MB and MI-GAN's code is MIT, though MI-GAN's weights inherit a lineage
  through three earlier generative models whose licence may cover code alone, which blocks an
  optional tier rather than the product.
- **One mirror survives the rule rather than being caught by it.** The optional Japanese reader's
  weights are an ONNX export by the same mirror as the inpainter's, three files of 343,454,249,
  117,480,262 and 30,216 bytes, and they are clean because their source is: the upstream model
  declares Apache-2.0, its training corpus is disclosed in the model card, and its own lineage is
  an Apache-2.0 vision encoder with an Apache-2.0 Japanese character decoder. There is no
  permissive re-tag of a restricted checkpoint here, only a conversion of a permissive model.
  Traced to the training source, as the rule requires, and not accepted because the mirror said so.
- **A Manga109-trained detector would have answered several open questions and is not takeable.**
  The published ones are Ultralytics YOLO, so AGPL-3.0 through the training framework whatever
  their own tag says, and the dataset itself is licence-gated, which blocks training a replacement
  as surely as the framework blocks using theirs. Two separate reasons, either one sufficient.
- **Exemplar synthesis was removed on patent grounds.** The PatchMatch patent has a 2008
  priority and is active until 2031, with a family member to 2030, and the reference
  implementation is licensed for non-commercial research only. Reimplementing it in another
  language does not help: clean-room defeats copyright, not patents, so treating a rewrite as
  risk elimination was risk conversion, from a build problem into a legal one.
- **One cloud provider's terms are incompatible with the users' own inputs.** They take a fully
  paid, royalty-free, perpetual, irrevocable, worldwide and sublicensable licence over inputs
  and outputs and state they may be used for training, with opt-out only by manual email. Users'
  inputs are third-party copyrighted scans and they cannot grant that licence.
- **The only viable Rust PSD writer is dangerous rather than merely immature.** One star, two
  commits, no CI, 215 downloads, an author who states the code was machine-generated and never
  hand-audited, and a fixture round-trip harness excluded from the published crate, so its own
  test suite never round-trips a file. Its write path panics on at least five conditions instead
  of returning an error, and its run-length writer silently drops out-of-bounds writes with call
  sites defaulting on failure, so an encode failure produces an empty channel: a corrupt file
  that reports success. Its pixel container is interleaved 8-bit with a stride hardcoded in three
  functions, so 16-bit would have been a fork rather than a feature. It was not taken: the format
  is written in the core instead, which cost less than hardening the crate and gave every mode and
  both depths rather than a hardened 8-bit path with 16-bit deferred.
- **The image library was reversed on its own headline requirement.** The C library first chosen
  hard-codes indexed images to RGB at load, so the palette is destroyed before anything else
  happens. The rest of the case collapsed with it: about 18 MB per platform (17.78 MB on
  darwin-arm64, 19.15 MB on win32-x64), 29 transitive libraries mostly dead weight for PNG and
  TIFF, LGPL components that put static linking off the table, and a binding whose own README
  warns its image type is not thread safe.

## Things that turned out wrong

- **"The CoreML provider cannot run LaMa."** It runs, 53% slower, after a 23.7 s session build.
  The conclusion held and the reason did not, and a reader designs around a hard failure.
- **"The inpainter is CPU-only and that is accepted."** It is 1.9× faster on WebGPU.
- **"MI-GAN is the CoreML-accelerated fast tier."** CoreML makes it 1.6× slower; it is fast
  because it is 28 MB. "No export work" was wrong too: the published file is a pipeline.
- **"WebGPU ships in the stock runtime build on every platform."** True of the one build
  measured, written in the same session as a warning against generalising a one-platform figure.
- **"`libloading` opens the runtime with `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR`, so `DirectML.dll`
  is found beside it."** It does not: `libloading` 0.9.0's `Library::new` is
  `load_with_flags(filename, 0)`, flags zero. What actually resolved `DirectML.dll` was the
  `SetDllDirectoryW` call added for the WebGPU plugin, working by accident on a delay-loaded
  import. A comment naming the wrong mechanism is worse than none, because the call it depends on
  reads as removable. The dependency is now stated, and the library is pinned by absolute path so
  that resolution no longer rests on who owns the one directory slot `SetDllDirectoryW` holds.
- **"A 1600×2400 page is not downscaled at all."** It is scaled by 2.34 into a fixed 1024².
- **"The box classes are bubble, text bubble and free text."** Two language classes; the three
  names belonged to a different model.
- **A memory budget of about 250 MB and 500 MB.** Measured at 2.07 GB and 2.45 GB, and it then
  turned out to exist in three places, only one of which was ever corrected.
- **25 GB for the local redraw model.** That measured how a caller drove the model: the
  reference implementation keeps its memory discipline in a callback only its command-line front
  end registers, so a library caller gets an unbounded cache and an encoder that is never evicted.
- **2.89 GB for the same rung once it was built here.** The wrong instrument: 6.58 GB. And the
  guards were claimed at 8.6×, a ratio between two different quantities, rather than 2.3×.
- **An 8,768 ms render.** It has never reproduced; six runs land between 19.5 s and 24.4 s.
- **The per-process available-memory call as the pressure signal.** It returns 0 for every
  process on the machine.
- **The fitted mask as the hole handed to a model.** On flat paper that is 355% of the detection
  box, and it produced a visible light box in the panel.
- **"Screentone is far below the strong-edge threshold."** A quarter of it is above, and the
  rule that comment explains works for the opposite reason.
- **"Outside-mask pixels never leave the machine" for the cloud rung.** False: the tone fit and
  the drift gate need the ring, so a bounded crop including surrounding art is transmitted.
- **"A stroke-tight hole leaves the least ink."** True of eleven fixture regions and false on the
  first real scan it met, where the tight hole is what the model draws a glyph into.
- **"The only viable Rust PSD writer must therefore be vendored."** The format this application
  needs is a few hundred lines, and vendoring would have bought less for more.
- **"The small-box drop only catches speckle."** It caught seven lines of dialogue on twelve pages,
  each of them inside a balloon the balloon detector was sure of.
- **"A bubble box is a weak answer and a paper reading may overrule it."** Not when the sure bubble
  boxes cover the region between them, which is what a balloon drawn as two lobes looks like.
- **"The ring walk can always answer the paper question."** Not for a box whose frame sits tight
  against its lettering, where there is no room for a band and the answer came back as picture.
- **"A Japanese-only reader has no place in the design."** True of the gate and false of the gate's
  failures, which are a reading problem and not a classification one.
- **"A CUDA flavour puts the inpainter back on the CPU."** Not since the WebGPU plugin ships
  beside every Windows and Linux x64 runtime.

## What has not been measured

- **Nothing has been executed on Windows or Linux.** Every DirectML and CUDA entry is an
  unmeasured candidate, reported as such, with no measured peak, so the memory gate never fires
  on one. The same holds for custom-protocol throughput there: no number is offered and the tile
  constant stays provisional, because measuring it on a Mac would reproduce a recorded error.
- **The Windows repairs of this round are all arguments, not runs.** Each is sourced from a
  binary or a vendored crate rather than from a machine, and each names the observation it rests
  on: that bundled resources land in the executable's directory and are therefore first in the
  loader's search order, so the staged Visual C++ libraries are found; that `LoadLibraryExW`
  answers 126 for a present file whose own imports are missing, which is what separates a missing
  dependency from a missing runtime; that `GetEpDevices` lists a DirectML device on a build whose
  RTTI carries `DmlEpFactory`, which is what lets DirectML be declined on a machine with no
  Direct3D 12 adapter instead of failing a session build to find out; that a mapped DLL may be
  renamed within its directory although it may not be deleted, which is what makes a runtime
  replaceable after the first install; and that `SetNamedSecurityInfoW` narrows the settings
  file's DACL. The first CI run on `windows-latest` is what tests the compile half. Nothing tests
  the rest but a Windows machine.
- **The WebGPU plugin on the two platforms it was added for.** Direct3D 12 on Windows and Vulkan
  on Linux, where the only measurement is Metal's: every automatic GPU choice off macOS is
  reported as unmeasured, and a forced WebGPU is still gated against a peak taken on a different
  backend. The loader half is the sharper hole. The runtime's directory is put on the library
  search path so that the two shader libraries the plugin loads by name resolve from the download
  folder, which is what the documented search order says will happen and what no Windows machine
  has confirmed; if it does not, the plugin registers, enumerates no device, and the row blames
  the missing adapter.
- **A quality comparison of the inpainter on screentone.** No published comparison exists and the
  corpus with ground-truth masks has not been run, so there is no manga quality evidence for the
  optional redraw sidecar; it is offered and not recommended. The comparison the removed fast tier
  was the other half of is now history rather than an open question, and so is the question of its
  blend ring.
- **The PSD writer against real Photoshop.** An independent reader opens every fixture, layered
  and flat, at 8 and 16 bits; Photoshop has opened none, and the two rules a second reader would
  not have caught, the 16-bit layer block and inverted CMYK, are followed from the specification
  rather than from a file Photoshop wrote. No text layer is written at all, so the vertical-text
  hazard this item once named is moot. Beside it: the large-document variant is not written and a
  page over 30 000 px a side is refused rather than promoted to it; the 2 GB file-size ceiling is
  computable before the first byte and is not computed, so a large layered set could produce a
  file Photoshop refuses and be reported as exported; a stitched PSD is refused although a
  row-streamed background would make it possible; an indexed or sub-8-bit page is refused rather
  than promoted, because the statement a promotion would need has nowhere to reach the interface;
  and the file carries no provenance record, so a typesetter cannot tell from it which engine
  cleaned which region.
- **A chapter written before pages were imported into the library is not migrated,** and behaves
  exactly as it did, including breaking if the scan folder goes. Nothing hashes the original a
  converted page was made from either, so replacing that original after the chapter exists is
  neither honoured nor reported, and there is no free-space check in front of the doubled write.
- **The rescue's thresholds are floors with reasons rather than measurements.** All twelve rescues
  came back fully Japanese and every reading that failed the share test failed it at zero, so the
  whole evidence is two clusters with the band between them empty and the threshold placed in the
  middle of nothing. What it guards against is a reader hallucinating a few characters over
  lettering it cannot read, and nobody has produced that case: the honest way to get one is a scan
  with English lettering inside a drawn balloon that the identifier fails on, which none of the 28
  reference pages contains. The two-character length floor counts punctuation the share test
  abstains on, so one rescue clears a floor meant to mean two characters of evidence; tightening
  it would also refuse two genuine one-kana balloons, which are common in manga. The reading's own
  score is carried for exactly this argument and nothing routes on it. The reader decodes greedily
  where the reference uses four beams, and stops at 64 tokens, which nothing has reached.
- **The adoption floor is untested from below,** reused from the score the balloon question already
  calls sure although the two questions differ in what a wrong yes costs: painting over art there,
  a review row here. The lowest box adopted scored 0.53 and nothing between 0.5 and 0.53 was seen
  either way. No threshold seed fallback was written for an adopted box with no segmentation under
  it, because every one of them had segmentation; and a box straddling a segment cut is kept only
  where it lies wholly inside a crop, so one taller than the detection overlap is adopted by
  neither, which only a long strip could produce and no strip fixture does.
- **The inner paper reading's margin is two points wide on one scan set,** 28 pages of one title
  screened at one line count: the narration boxes reach 14 per cent, the allowance is 16 and the
  nearest thing that must not pass is at 19. A finer or lighter tone puts fewer dots off a capped
  tolerance and a box that is mostly frame puts more, and neither has been seen. The direction of
  the error is the safe one, since too high admits tone and too low sends narration back to review.
  The reading also cannot tell a hand-lettered effect on white paper from narration on white
  paper, because between the strokes there is the same white: two such effects moved to uncertain,
  where they are still held rather than painted. The difference is in the lettering, and nothing
  in this module looks at the lettering.
- **Caption boxes and spiky balloons the balloon detector labels free text** are still held back on
  noisy scans: narration whose interior spread runs past even the widened tolerance, and spiky
  balloons over screentone whose first rings are already tone. The opt-in cleans them; without it
  they are Clean anyway rows. The strip test that decides whether a merge crosses a balloon still
  reads against the old fixed tolerance, which on the same scans over-reports boundaries and
  under-merges.
- **A Manga109-trained detector,** which would have answered the free-text and sound-effect
  questions directly, is blocked twice over: the published ones are AGPL through their training
  framework, and the dataset that would let one be trained here is licence-gated.
- **An optional model that will not open is silent.** The reader is attached when its three files
  are present and the gate falls back without it when opening fails, on a truncated download, a
  vocabulary that does not match, or a provider that refuses the graph. That is the right
  behaviour and the wrong reporting, against the rule that every refusal is named. The failure has
  not been observed.
- **The redraw sidecar's return to its floor between regions,** instrumented and never judged
  for want of a threshold. Nor does its allocator fail fast: the limit is documented upstream as
  a guideline, and a 1024² render exceeded a 12 GB limit by 440 MB and completed normally. What
  is real is the parent's per-region deadline, hang kill and peak check, one region coarser.
- **The second redraw backend on CUDA,** the platform it exists for: the weights would land in
  video memory and the host-side figure this rung declares is not the one that would bind. The
  crop's 24 to 80 px context clamp is unswept too, unlike the resolution and padding beside it.
- **The disagreement between two harnesses on one machine,** 1.7× to 2.1× on the same models.
  The in-process rung peaks came off the same tool that produced the wrong sidecar figure and
  may be the same wrong line of its output.
