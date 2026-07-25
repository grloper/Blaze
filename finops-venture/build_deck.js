const pptxgen = require("pptxgenjs");

const pres = new pptxgen();
pres.layout = "LAYOUT_WIDE"; // 13.33 x 7.5
pres.author = "FinOps Micro-Agency";
pres.title = "Found Money";

// ---- Palette: navy dominant, money-green accent ----
const NAVY = "0B1B2B";
const NAVY2 = "13304A";
const SLATE = "1C4A6E";
const GREEN = "2ECC8F";
const GREEN_DK = "178C5F";
const WHITE = "FFFFFF";
const BG = "F4F7FA";
const TEXT = "0B1B2B";
const MUTED = "5A7183";
const MUTED_LT = "9DB2C4";
const BORDER = "DCE5ED";

const H = "Cambria";   // headers
const B = "Calibri";   // body

const W = 13.33;
const M = 0.7;         // page margin
const CW = W - M * 2;  // content width

// ---- Grid: every row of cards must land inside CW exactly ----
const G  = 0.45;
const G4 = 0.36;
const c3 = (CW - 2 * G) / 3;    // 3.677
const c2 = (CW - G) / 2;        // 5.740
const c4 = (CW - 3 * G4) / 4;   // 2.713
const cx3 = (i) => M + i * (c3 + G);
const cx2 = (i) => M + i * (c2 + G);
const cx4 = (i) => M + i * (c4 + G4);

const card = () => ({ type: "outer", color: "9AAEC0", blur: 12, offset: 3, angle: 90, opacity: 0.18 });

function darkBg(s) { s.background = { color: NAVY }; }
function lightBg(s) { s.background = { color: BG }; }

function title(s, text, color) {
  s.addText(text, {
    x: M, y: 0.5, w: CW, h: 1.0,
    fontFace: H, fontSize: 34, bold: true,
    color: color || TEXT, align: "left", margin: 0,
  });
}

function kicker(s, text, color) {
  s.addText(text, {
    x: M, y: 0.24, w: CW, h: 0.3,
    fontFace: B, fontSize: 12, bold: true, charSpacing: 2,
    color: color || GREEN_DK, align: "left", margin: 0,
  });
}

/* ============================ SLIDE 1 — TITLE ============================ */
{
  const s = pres.addSlide();
  darkBg(s);

  s.addText("THE BUSINESS IN ONE SLIDE", {
    x: M, y: 1.25, w: 7.4, h: 0.3,
    fontFace: B, fontSize: 12, bold: true, charSpacing: 2, color: GREEN, margin: 0,
  });

  s.addText("The money is\nalready there.", {
    x: M, y: 1.75, w: 7.4, h: 2.1,
    fontFace: H, fontSize: 52, bold: true, color: WHITE, lineSpacing: 54, margin: 0,
  });

  s.addText(
    "We find the fifth of a funded startup's cloud bill that nobody is watching, " +
    "hand it back as cash, and get paid a slice of what we recover.",
    { x: M, y: 4.05, w: 7.1, h: 1.1, fontFace: B, fontSize: 17, color: MUTED_LT, lineSpacing: 26, margin: 0 }
  );

  s.addText("No product to build.  No inventory.  No upfront capital.", {
    x: M, y: 5.35, w: 7.1, h: 0.4,
    fontFace: B, fontSize: 15, bold: true, italic: true, color: GREEN, margin: 0,
  });

  // Stat badge
  s.addShape(pres.ShapeType.ellipse, {
    x: 8.85, y: 1.9, w: 3.5, h: 3.5,
    fill: { color: NAVY2 }, line: { color: GREEN, width: 2.25 },
  });
  s.addText("20–35%", {
    x: 8.85, y: 2.75, w: 3.5, h: 0.95,
    fontFace: H, fontSize: 44, bold: true, color: GREEN, align: "center", margin: 0,
  });
  s.addText("of the average AWS bill\nis recoverable waste", {
    x: 8.95, y: 3.7, w: 3.3, h: 0.8,
    fontFace: B, fontSize: 13, color: MUTED_LT, align: "center", lineSpacing: 18, margin: 0,
  });

  s.addNotes(
    "Open with the premise, not the mechanics. Every funded startup is overspending on cloud infrastructure " +
    "by roughly a fifth to a third. That is not a rumour — it is measurable inside their own account in about " +
    "thirty minutes. We are not inventing a product or a market. The money already exists, it is already " +
    "theirs, and nobody is looking at it. Our entire business is being the people who look. " +
    "Stress the last line: this business needs no capital, no inventory, and nothing built before the first " +
    "sale. That is what makes it the right first move for four people with skills and no money."
  );
}

/* ==================== SLIDE 2 — THE UNSPOKEN PROBLEM ==================== */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "THE PROBLEM");
  title(s, "Startups burn $50,000+ a year without noticing");

  const stats = [
    { v: "$20k", l: "Typical monthly AWS bill\nat a Series A startup", c: SLATE },
    { v: "20–35%", l: "Of that spend is waste —\nnot mistakes, just nobody's job", c: GREEN_DK },
    { v: "$50–80k", l: "Recoverable every year,\nfrom one company", c: SLATE },
  ];
  const cw = c3, gap = G;
  stats.forEach((st, i) => {
    const x = M + i * (cw + gap);
    s.addShape(pres.ShapeType.roundRect, {
      x, y: 1.72, w: cw, h: 2.5, rectRadius: 0.09,
      fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
    });
    s.addText(st.v, {
      x, y: 2.02, w: cw, h: 0.95,
      fontFace: H, fontSize: 42, bold: true, color: st.c, align: "center", margin: 0,
    });
    s.addText(st.l, {
      x: x + 0.28, y: 3.0, w: cw - 0.56, h: 0.95,
      fontFace: B, fontSize: 13.5, color: MUTED, align: "center", lineSpacing: 19, margin: 0,
    });
  });

  s.addShape(pres.ShapeType.roundRect, {
    x: M, y: 4.55, w: CW, h: 1.95, rectRadius: 0.09,
    fill: { color: NAVY }, line: { color: NAVY, width: 1 },
  });
  s.addText("Why it happens", {
    x: M + 0.45, y: 4.82, w: 4, h: 0.35,
    fontFace: B, fontSize: 12, bold: true, charSpacing: 2, color: GREEN, margin: 0,
  });
  s.addText(
    "Nothing dramatic. A test server from last quarter nobody switched off. Storage left behind when a " +
    "machine was deleted. Development environments running all night and all weekend for nobody. " +
    "A discount programme the provider offers that nobody signed up for.",
    { x: M + 0.45, y: 5.2, w: CW - 0.9, h: 1.1, fontFace: B, fontSize: 15, color: "C8D6E2", lineSpacing: 23, margin: 0 }
  );

  s.addNotes(
    "The key idea for a non-technical listener: this is not incompetence and you must never pitch it as such. " +
    "It is the natural result of a fast-growing company where infrastructure gets created constantly and " +
    "deleted never. Use the physical analogy if the room needs it: it is like a company renting twelve " +
    "offices, using four of them, and nobody ever cancelling the other eight — because cancelling a lease is " +
    "somebody's job and nobody was ever given it. " +
    "The three numbers are the whole slide. A $20k monthly bill is completely ordinary at Series A. " +
    "A fifth to a third of it is waste. That is $50-80k a year, from a single customer, sitting there."
  );
}

/* ================= SLIDE 3 — WHY ENGINEERS DON'T FIX IT ================= */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "THE OPENING");
  title(s, "Their engineers could fix it. They won't.");

  const rows = [
    { n: "1", h: "Nobody owns the bill", d: "It arrives, the boss winces, everyone moves on. It is no single person's responsibility, so it is nobody's." },
    { n: "2", h: "It is never this sprint's priority", d: "Every engineer is shipping features customers asked for. Cleanup always loses that argument, every single week." },
    { n: "3", h: "Nobody gets promoted for it", d: "You get promoted for launching things. There is no career reward anywhere for making the bill smaller." },
    { n: "4", h: "Deleting things is frightening", d: "\"Is this still in use? Who made it? If I remove it and the product breaks at 3am, is that on me?\" So it stays." },
  ];

  const cw = c2, ch = 1.72, gx = G, gy = 0.32;
  rows.forEach((r, i) => {
    const x = M + (i % 2) * (cw + gx);
    const y = 1.68 + Math.floor(i / 2) * (ch + gy);
    s.addShape(pres.ShapeType.roundRect, {
      x, y, w: cw, h: ch, rectRadius: 0.09,
      fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
    });
    s.addShape(pres.ShapeType.ellipse, {
      x: x + 0.32, y: y + 0.36, w: 0.62, h: 0.62, fill: { color: NAVY }, line: { color: NAVY, width: 1 },
    });
    s.addText(r.n, {
      x: x + 0.32, y: y + 0.44, w: 0.62, h: 0.42,
      fontFace: H, fontSize: 20, bold: true, color: GREEN, align: "center", margin: 0,
    });
    s.addText(r.h, {
      x: x + 1.12, y: y + 0.3, w: cw - 1.45, h: 0.38,
      fontFace: B, fontSize: 16.5, bold: true, color: TEXT, margin: 0,
    });
    s.addText(r.d, {
      x: x + 1.12, y: y + 0.72, w: cw - 1.45, h: 0.85,
      fontFace: B, fontSize: 12.5, color: MUTED, lineSpacing: 17, margin: 0,
    });
  });

  s.addText(
    "This is the opening. The problem is not a lack of skill — it is that fixing it is somebody's risk and nobody's reward.",
    { x: M, y: 5.78, w: CW, h: 0.5, fontFace: B, fontSize: 15.5, bold: true, italic: true, color: GREEN_DK, align: "center", margin: 0 }
  );

  s.addNotes(
    "This is the most important slide in the deck, because it answers the question every listener is silently " +
    "asking: if this money is just lying there, why hasn't their own team picked it up? " +
    "Never suggest their engineers are not good enough. They usually are. The barrier is entirely structural — " +
    "reason four is the real one. An engineer who deletes something and causes an outage owns that outage " +
    "forever. An engineer who leaves it running loses the company $400 a month and nobody ever finds out. " +
    "Given those incentives, leaving it running is the rational choice every time. " +
    "That gap is our entire business. We are the outside party who does the frightening, boring sweep and " +
    "carries the risk of having done it."
  );
}

/* ==================== SLIDE 4 — WHAT WE ARE SELLING ==================== */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "POSITIONING");
  title(s, "We sell found money, not cloud efficiency");

  const cols = [
    {
      x: cx2(0), head: "✕   \"Cloud efficiency\"", headC: "A03A3A", bg: WHITE, bd: BORDER,
      bodyC: MUTED, headline: "What everyone else pitches",
      pts: ["Abstract — nobody has an efficiency budget", "Sounds technical, so it routes to engineering", "Reads as a nice-to-have, and gets deferred", "Competes with the product roadmap and loses"],
    },
    {
      x: cx2(1), head: "✓   \"Found money\"", headC: GREEN, bg: NAVY, bd: NAVY,
      bodyC: "C8D6E2", headline: "What we pitch",
      pts: ["Concrete — a number with a currency sign", "Financial, so the founder engages directly", "Pays for itself, so approval is trivial", "Extends runway, which is the only metric that matters"],
    },
  ];

  cols.forEach((c) => {
    s.addShape(pres.ShapeType.roundRect, {
      x: c.x, y: 1.7, w: c2, h: 3.55, rectRadius: 0.1,
      fill: { color: c.bg }, line: { color: c.bd, width: 1 }, shadow: card(),
    });
    s.addText(c.headline, {
      x: c.x + 0.45, y: 1.95, w: c2 - 0.9, h: 0.3,
      fontFace: B, fontSize: 11.5, bold: true, charSpacing: 1.5,
      color: c.bg === NAVY ? MUTED_LT : MUTED, margin: 0,
    });
    s.addText(c.head, {
      x: c.x + 0.45, y: 2.3, w: c2 - 0.9, h: 0.5,
      fontFace: H, fontSize: 26, bold: true, color: c.headC, margin: 0,
    });
    s.addText(
      c.pts.map((p, i) => ({ text: p, options: { bullet: true, breakLine: i !== c.pts.length - 1 } })),
      { x: c.x + 0.5, y: 2.95, w: c2 - 0.95, h: 2.2, fontFace: B, fontSize: 13, color: c.bodyC, paraSpaceAfter: 9, lineSpacing: 18, margin: 0 }
    );
  });

  s.addShape(pres.ShapeType.roundRect, {
    x: M, y: 5.52, w: CW, h: 1.0, rectRadius: 0.09,
    fill: { color: "E4F5EC" }, line: { color: GREEN, width: 1.25 },
  });
  s.addText(
    "Same work. Same result. One of these gets a meeting and the other gets ignored.",
    { x: M, y: 5.77, w: CW, h: 0.5, fontFace: B, fontSize: 16.5, bold: true, color: GREEN_DK, align: "center", margin: 0 }
  );

  s.addNotes(
    "Two words for the identical piece of work, and only one of them sells. " +
    "\"Cloud efficiency\" is how a technical person naturally describes this, and it is a losing frame — " +
    "no company has an efficiency budget, so it competes against the product roadmap for attention and " +
    "always loses. \"Found money\" is not a euphemism or a trick; it is a more accurate description of what " +
    "the customer receives. They receive cash they already had. " +
    "The practical rule for the team: whoever writes the emails must never use the word efficiency, " +
    "optimisation, or rightsizing in a first message. Say the number and say the currency."
  );
}

/* ================== SLIDE 5 — WHERE THE MONEY HIDES ================== */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "THE PRODUCT");
  title(s, "Three places the money hides");

  const cats = [
    {
      tag: "QUICK WINS", tone: GREEN_DK, risk: "Zero risk · same week",
      items: ["Storage left behind by deleted machines", "Backups kept forever with no expiry rule", "Logs retained indefinitely by default", "Idle network addresses still being billed"],
      note: "$500–3,000 / month",
    },
    {
      tag: "EXECUTION WINS", tone: SLATE, risk: "Low risk · scheduled window",
      items: ["Machines far larger than the work needs", "Test environments running nights + weekends", "Unclaimed volume discounts (up to 66% off)", "Older, pricier hardware generations"],
      note: "The largest bucket",
    },
    {
      tag: "SNEAKY LEAKS", tone: "8A5A1E", risk: "Nobody ever looks here",
      items: ["Internal traffic charged as if external", "Data crossing zones that need not cross", "Duplicate backup and disaster-recovery tiers", "Test databases on full production tiers"],
      note: "Often the biggest surprise",
    },
  ];

  const cw = c3, gap = G;
  cats.forEach((c, i) => {
    const x = M + i * (cw + gap);
    s.addShape(pres.ShapeType.roundRect, {
      x, y: 1.68, w: cw, h: 4.02, rectRadius: 0.1,
      fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
    });
    s.addText(c.tag, {
      x: x + 0.34, y: 1.94, w: cw - 0.68, h: 0.32,
      fontFace: B, fontSize: 12, bold: true, charSpacing: 1.5, color: c.tone, margin: 0,
    });
    s.addText(c.risk, {
      x: x + 0.34, y: 2.26, w: cw - 0.68, h: 0.28,
      fontFace: B, fontSize: 11.5, italic: true, color: MUTED, margin: 0,
    });
    s.addText(
      c.items.map((t, j) => ({ text: t, options: { bullet: true, breakLine: j !== c.items.length - 1 } })),
      { x: x + 0.38, y: 2.68, w: cw - 0.72, h: 2.1, fontFace: B, fontSize: 12.5, color: TEXT, paraSpaceAfter: 8, lineSpacing: 17, margin: 0 }
    );
    s.addShape(pres.ShapeType.roundRect, {
      x: x + 0.34, y: 4.95, w: cw - 0.68, h: 0.5, rectRadius: 0.07,
      fill: { color: "EDF3F8" }, line: { color: "EDF3F8", width: 1 },
    });
    s.addText(c.note, {
      x: x + 0.34, y: 5.04, w: cw - 0.68, h: 0.32,
      fontFace: B, fontSize: 13, bold: true, color: c.tone, align: "center", margin: 0,
    });
  });

  s.addText(
    "We always do the zero-risk column first. Being right about the easy things is what buys permission to touch the hard ones.",
    { x: M, y: 5.95, w: CW, h: 0.5, fontFace: B, fontSize: 14.5, italic: true, color: MUTED, align: "center", margin: 0 }
  );

  s.addNotes(
    "Keep this deliberately non-technical. Column one is money lying on the floor — nothing is using these " +
    "resources at all, and removing them cannot affect anything. Column two needs a scheduled moment because " +
    "something briefly restarts, and it is normally the biggest single pile. Column three is the surprising " +
    "one: charges that look like ordinary infrastructure but are really the provider billing for internal " +
    "traffic that never needed to leave the building. " +
    "Note the sequencing line at the bottom, because it is a sales tactic and not just an engineering one. " +
    "We deliver the risk-free wins first. Once we have been demonstrably right about the safe items, the " +
    "customer trusts us with the changes that carry actual risk — and those are where the real money is."
  );
}

/* ==================== SLIDE 6 — THE WHEEL STRATEGY ==================== */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "OUR UNFAIR ADVANTAGE");
  title(s, "We didn't invent the wheel. We turn it.");

  s.addText(
    "Amazon already built tools that detect this waste automatically. They are free, they are switched on, " +
    "and they are sitting inside every customer's account right now. Almost nobody has ever opened them.",
    { x: M, y: 1.5, w: CW, h: 0.75, fontFace: B, fontSize: 15.5, color: MUTED, lineSpacing: 23, margin: 0 }
  );

  const tools = [
    { n: "Compute Optimizer", d: "Names every oversized machine", f: "Free · by Amazon" },
    { n: "Cost Optimization Hub", d: "Ranks savings by dollar value", f: "Free · by Amazon" },
    { n: "Trusted Advisor", d: "Flags idle and unused resources", f: "Free · by Amazon" },
    { n: "Cloud Custodian", d: "Enforces the rules automatically", f: "Free · open source" },
  ];
  const tw = c4, tgap = G4;
  tools.forEach((t, i) => {
    const x = M + i * (tw + tgap);
    s.addShape(pres.ShapeType.roundRect, {
      x, y: 2.42, w: tw, h: 1.62, rectRadius: 0.09,
      fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
    });
    s.addText(t.n, {
      x: x + 0.24, y: 2.62, w: tw - 0.48, h: 0.36,
      fontFace: B, fontSize: 13.5, bold: true, color: TEXT, margin: 0,
    });
    s.addText(t.d, {
      x: x + 0.24, y: 3.0, w: tw - 0.48, h: 0.5,
      fontFace: B, fontSize: 12, color: MUTED, lineSpacing: 16, margin: 0,
    });
    s.addText(t.f, {
      x: x + 0.24, y: 3.55, w: tw - 0.48, h: 0.28,
      fontFace: B, fontSize: 10.5, bold: true, color: GREEN_DK, margin: 0,
    });
  });

  s.addShape(pres.ShapeType.roundRect, {
    x: M, y: 4.35, w: CW, h: 2.1, rectRadius: 0.1,
    fill: { color: NAVY }, line: { color: NAVY, width: 1 },
  });
  s.addText("So what exactly are we paid for?", {
    x: M + 0.5, y: 4.6, w: 5.6, h: 0.42,
    fontFace: H, fontSize: 21, bold: true, color: WHITE, margin: 0,
  });
  s.addText(
    [
      { text: "Reading the output nobody opens", options: { bullet: true, breakLine: true } },
      { text: "Turning it into a decision, with a price tag on it", options: { bullet: true, breakLine: true } },
      { text: "Doing the work — and carrying the risk of having done it", options: { bullet: true, breakLine: false } },
    ],
    { x: M + 0.55, y: 5.12, w: 6.2, h: 1.15, fontFace: B, fontSize: 13.5, color: "C8D6E2", paraSpaceAfter: 6, lineSpacing: 18, margin: 0 }
  );

  s.addShape(pres.ShapeType.roundRect, {
    x: 7.55, y: 4.68, w: 5.08, h: 1.42, rectRadius: 0.09,
    fill: { color: NAVY2 }, line: { color: GREEN, width: 1.5 },
  });
  s.addText("A diagnosis is worthless.\nThe treatment is the product.", {
    x: 7.75, y: 4.98, w: 4.68, h: 0.85,
    fontFace: H, fontSize: 17, bold: true, italic: true, color: GREEN, align: "center", lineSpacing: 24, margin: 0,
  });

  s.addNotes(
    "This is the strategic heart of the business and it maps exactly onto the reseller instinct — we are not " +
    "inventing anything, we are the delivery layer on top of tools somebody else already built and gave away. " +
    "The obvious objection: if the tools are free, why would anyone pay us? Use the medical analogy. A blood " +
    "test result is free to print and completely useless on its own. What you pay for is somebody who reads " +
    "it, tells you what it means, performs the procedure, and is accountable for the outcome. " +
    "Also worth saying to the team: because the detection is automated and free, our cost per audit is " +
    "essentially a few hours of attention. That is why the margins on this work are so unusually good."
  );
}

/* ================== SLIDE 7 — THE 48-HOUR SALES PITCH ================== */
{
  const s = pres.addSlide();
  darkBg(s);
  kicker(s, "THE OFFER", GREEN);
  title(s, "An offer that is impossible to refuse", WHITE);

  s.addShape(pres.ShapeType.roundRect, {
    x: M, y: 1.62, w: CW, h: 2.15, rectRadius: 0.12,
    fill: { color: NAVY2 }, line: { color: GREEN, width: 2.25 },
  });
  s.addText(
    "“Give us read-only access for 48 hours. We'll send back a document showing exactly where your money " +
    "is going. If we find less than $2,000 a year, you pay nothing.”",
    { x: M + 0.65, y: 1.92, w: CW - 1.3, h: 1.55, fontFace: H, fontSize: 23, bold: true, color: WHITE, align: "center", lineSpacing: 34, margin: 0 }
  );

  const why = [
    { h: "Nothing to lose", d: "No fee unless we find real money. The decision costs them nothing, so “no” has no argument behind it." },
    { h: "Nothing to install", d: "No software, no migration, no engineering time. One permission, granted in about five minutes." },
    { h: "Nothing to sign off", d: "Read-only. We cannot change, delete, or break anything — the permission does not allow it." },
    { h: "Nothing to unwind", d: "They revoke it by deleting one setting. No contract to exit, no offboarding, no lock-in." },
  ];
  const cw = c4, gap = G4;
  why.forEach((w, i) => {
    const x = M + i * (cw + gap);
    s.addShape(pres.ShapeType.roundRect, {
      x, y: 4.05, w: cw, h: 1.92, rectRadius: 0.09,
      fill: { color: "12283C" }, line: { color: SLATE, width: 1 },
    });
    s.addText(w.h, {
      x: x + 0.26, y: 4.28, w: cw - 0.52, h: 0.34,
      fontFace: B, fontSize: 14.5, bold: true, color: GREEN, margin: 0,
    });
    s.addText(w.d, {
      x: x + 0.26, y: 4.66, w: cw - 0.52, h: 1.15,
      fontFace: B, fontSize: 11.5, color: "B4C6D6", lineSpacing: 16, margin: 0,
    });
  });

  s.addText(
    "We are not asking them to buy anything. We are asking them to let us look.",
    { x: M, y: 6.24, w: CW, h: 0.45, fontFace: B, fontSize: 15, bold: true, italic: true, color: GREEN, align: "center", margin: 0 }
  );

  s.addNotes(
    "This is the sentence the whole business runs on, so rehearse it until it is automatic. " +
    "The reason it works is that it removes every reason to say no, one at a time. There is no cost, no " +
    "installation, no engineering time, no risk of damage, and no commitment. What is left is a person " +
    "deciding whether to spend five minutes to find out a number about their own company. That is a very " +
    "easy yes, and it is deliberately not a buying decision at all. " +
    "One critical framing note for whoever is doing outreach: we never ask for a meeting in the first " +
    "message. We ask for permission to look. The meeting happens naturally once there is a real number to " +
    "discuss, and by then we are not selling — we are reporting findings they asked for."
  );
}

/* ==================== SLIDE 8 — HOW WE EXECUTE ==================== */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "DELIVERY");
  title(s, "Three steps, and we never touch the switch");

  const steps = [
    { n: "1", t: "Read-only audit", d: "They grant a viewing permission that cannot alter anything. We run the free detection tools and read every result.", time: "Day 1" },
    { n: "2", t: "Findings matrix", d: "One document. Every item of waste, its dollar value, how risky removing it is, and how long it takes.", time: "Day 2" },
    { n: "3", t: "Safe implementation", d: "Each change is proposed as a written request their own engineer reviews and approves. They press the button, never us.", time: "Week 2+" },
  ];

  const cw = c3, gap = G;
  steps.forEach((st, i) => {
    const x = M + i * (cw + gap);
    s.addShape(pres.ShapeType.roundRect, {
      x, y: 1.78, w: cw, h: 2.72, rectRadius: 0.1,
      fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
    });
    s.addShape(pres.ShapeType.ellipse, {
      x: x + 0.34, y: 2.04, w: 0.7, h: 0.7, fill: { color: GREEN }, line: { color: GREEN, width: 1 },
    });
    s.addText(st.n, {
      x: x + 0.34, y: 2.14, w: 0.7, h: 0.48,
      fontFace: H, fontSize: 23, bold: true, color: WHITE, align: "center", margin: 0,
    });
    s.addText(st.time, {
      x: x + 1.2, y: 2.12, w: cw - 1.5, h: 0.28,
      fontFace: B, fontSize: 11, bold: true, charSpacing: 1.2, color: MUTED, margin: 0,
    });
    s.addText(st.t, {
      x: x + 1.2, y: 2.4, w: cw - 1.5, h: 0.36,
      fontFace: B, fontSize: 16, bold: true, color: TEXT, margin: 0,
    });
    s.addText(st.d, {
      x: x + 0.34, y: 3.0, w: cw - 0.68, h: 1.28,
      fontFace: B, fontSize: 12.5, color: MUTED, lineSpacing: 18, margin: 0,
    });

    if (i < 2) {
      s.addShape(pres.ShapeType.chevron, {
        x: x + cw + 0.06, y: 2.97, w: 0.32, h: 0.34,
        fill: { color: MUTED_LT }, line: { color: MUTED_LT, width: 1 },
      });
    }
  });

  s.addShape(pres.ShapeType.roundRect, {
    x: M, y: 4.82, w: CW, h: 1.68, rectRadius: 0.1,
    fill: { color: NAVY }, line: { color: NAVY, width: 1 },
  });
  s.addText("The rule that protects the whole business", {
    x: M + 0.5, y: 5.05, w: CW - 1, h: 0.34,
    fontFace: B, fontSize: 12, bold: true, charSpacing: 1.5, color: GREEN, margin: 0,
  });
  s.addText(
    "We never hold permission to change a customer's live system. Not once, not briefly, not as a favour. " +
    "We recommend; they approve and execute. It is our strongest legal protection and, conveniently, also " +
    "the answer to the only objection that ever really matters.",
    { x: M + 0.5, y: 5.42, w: CW - 1, h: 0.95, fontFace: B, fontSize: 14.5, color: "C8D6E2", lineSpacing: 22, margin: 0 }
  );

  s.addNotes(
    "Walk the three steps quickly — the audience does not need the technical detail, only the shape. " +
    "Then slow right down for the box at the bottom, because it is the single most important operating rule " +
    "in the company and it does two jobs at once. " +
    "Commercially, it dissolves the objection every prospect raises: what if you break something? We " +
    "cannot. We do not have the ability, by design, and we will show them the permission list that proves it. " +
    "Legally, it means every change was reviewed and approved in writing by the customer's own engineer " +
    "before it happened. There is a permanent record of shared decision-making. " +
    "If anyone on the team is ever tempted to accept full access because it would be faster, the answer is " +
    "no. The moment we hold that access, every unrelated outage in that account becomes a conversation " +
    "about us."
  );
}

/* ================ SLIDE 9 — BUSINESS MODEL & PRICING ================ */
{
  const s = pres.addSlide();
  lightBg(s);
  kicker(s, "HOW WE GET PAID");
  title(s, "Audits acquire. Retainers are the business.");

  // Left: the ladder
  s.addShape(pres.ShapeType.roundRect, {
    x: cx2(0), y: 1.68, w: c2, h: 4.1, rectRadius: 0.1,
    fill: { color: WHITE }, line: { color: BORDER, width: 1 }, shadow: card(),
  });
  s.addText("Stage 1  ·  The audit", {
    x: cx2(0) + 0.42, y: 1.94, w: c2 - 0.84, h: 0.36,
    fontFace: B, fontSize: 15, bold: true, color: TEXT, margin: 0,
  });

  const ladder = [
    { a: "Clients 1–2", b: "$1,000 flat", c: "Priced to be an easy yes. You are buying a testimonial and a reference, not profit." },
    { a: "Clients 3–5", b: "$5,000–15,000 flat", c: "Same work, real price. By now you have proof and a process." },
    { a: "Clients 6+", b: "20–30% of savings", c: "Only once you can measure savings properly and have contracts to back it." },
  ];
  ladder.forEach((l, i) => {
    const y = 2.42 + i * 1.08;
    s.addText(l.a, { x: cx2(0) + 0.42, y, w: 1.75, h: 0.3, fontFace: B, fontSize: 12.5, bold: true, color: MUTED, margin: 0 });
    s.addText(l.b, { x: cx2(0) + 2.2, y: y - 0.03, w: 3.3, h: 0.34, fontFace: B, fontSize: 16, bold: true, color: GREEN_DK, margin: 0 });
    s.addText(l.c, { x: cx2(0) + 0.42, y: y + 0.32, w: c2 - 0.84, h: 0.6, fontFace: B, fontSize: 11.5, color: MUTED, lineSpacing: 16, margin: 0 });
  });

  s.addText(
    "Flat fees first. Percentage deals start arguments.",
    { x: cx2(0) + 0.42, y: 5.46, w: c2 - 0.84, h: 0.28, fontFace: B, fontSize: 11.5, bold: true, italic: true, color: "A03A3A", margin: 0 }
  );

  // Right: retainers
  s.addShape(pres.ShapeType.roundRect, {
    x: cx2(1), y: 1.68, w: c2, h: 4.1, rectRadius: 0.1,
    fill: { color: NAVY }, line: { color: NAVY, width: 1 }, shadow: card(),
  });
  s.addText("Stage 2  ·  The retainer", {
    x: cx2(1) + 0.42, y: 1.94, w: c2 - 0.84, h: 0.36,
    fontFace: B, fontSize: 15, bold: true, color: WHITE, margin: 0,
  });
  s.addText("Waste grows back in 6–9 months. We stop it.", {
    x: cx2(1) + 0.42, y: 2.3, w: c2 - 0.84, h: 0.3,
    fontFace: B, fontSize: 12, italic: true, color: MUTED_LT, margin: 0,
  });

  const tiers = [
    { n: "Watch", p: "$1,500 / mo", d: "Monthly report, alerts, budget management" },
    { n: "Guard", p: "$3,000 / mo", d: "Adds automated guardrails + quarterly re-audit" },
    { n: "Embedded", p: "$5,000+ / mo", d: "Adds implementation hours + architecture review" },
  ];
  tiers.forEach((t, i) => {
    const y = 2.76 + i * 0.85;
    s.addShape(pres.ShapeType.roundRect, {
      x: cx2(1) + 0.42, y, w: c2 - 0.84, h: 0.72, rectRadius: 0.07,
      fill: { color: "12283C" }, line: { color: SLATE, width: 1 },
    });
    s.addText(t.n, { x: cx2(1) + 0.62, y: y + 0.08, w: 1.5, h: 0.3, fontFace: B, fontSize: 14, bold: true, color: WHITE, margin: 0 });
    s.addText(t.p, { x: cx2(1) + 2.1, y: y + 0.08, w: 1.8, h: 0.3, fontFace: B, fontSize: 14, bold: true, color: GREEN, margin: 0 });
    s.addText(t.d, { x: cx2(1) + 0.62, y: y + 0.38, w: c2 - 1.24, h: 0.28, fontFace: B, fontSize: 10.5, color: MUTED_LT, margin: 0 });
  });

  s.addText(
    "Keep it under 10% of the savings it protects.",
    { x: cx2(1) + 0.42, y: 5.46, w: c2 - 0.84, h: 0.28, fontFace: B, fontSize: 11.5, bold: true, italic: true, color: GREEN, margin: 0 }
  );

  s.addText(
    "“The audit gets the money back once. The retainer is what stops it coming back.”",
    { x: M, y: 6.05, w: CW, h: 0.45, fontFace: H, fontSize: 17, bold: true, italic: true, color: TEXT, align: "center", margin: 0 }
  );

  s.addNotes(
    "Two revenue lines and they do completely different jobs. The audit is how we win a customer — it is " +
    "deliberately underpriced at the start because the first two are buying us proof, references, and the " +
    "right to charge properly afterwards. " +
    "Explain the red warning honestly: taking a percentage of savings sounds far more attractive and it is " +
    "how the mature firms in this market operate, but it requires being able to prove precisely what was " +
    "saved months after the fact. Until we have that measurement discipline and proper contracts, a " +
    "percentage deal turns into an argument at exactly the moment we most need a reference. Flat fees first. " +
    "The right-hand side is the actual business. Ten retainers at two thousand a month is twenty thousand " +
    "recurring for roughly forty to sixty hours of work. Audit-only revenue is a job — you start from zero " +
    "every single month. Retainers are an asset."
  );
}

/* ================ SLIDE 10 — GROWTH & PROJECTION ================ */
{
  const s = pres.addSlide();
  darkBg(s);
  kicker(s, "THE PATH", GREEN);
  title(s, "From one client to $20k a month", WHITE);

  s.addChart(
    pres.ChartType.bar,
    [{
      name: "Monthly revenue",
      labels: ["Month 1", "Month 3", "Month 6", "Month 12"],
      values: [1000, 6500, 18000, 32000],
    }],
    {
      x: M, y: 1.62, w: 7.5, h: 4.15,
      barDir: "col",
      chartColors: [GREEN],
      showTitle: false,
      showLegend: false,
      showValue: true,
      dataLabelPosition: "outEnd",
      dataLabelColor: WHITE,
      dataLabelFontFace: B,
      dataLabelFontSize: 12,
      dataLabelFormatCode: '$#,##0',
      catAxisLabelColor: MUTED_LT,
      catAxisLabelFontFace: B,
      catAxisLabelFontSize: 12,
      valAxisLabelColor: MUTED_LT,
      valAxisLabelFontFace: B,
      valAxisLabelFontSize: 10,
      valAxisMaxVal: 38000,
      valGridLine: { color: "1E3A52", size: 1 },
      catGridLine: { style: "none" },
      barGapWidthPct: 55,
      chartArea: { fill: { color: NAVY } },
      plotArea: { fill: { color: NAVY } },
    }
  );

  const miles = [
    { m: "Month 1", d: "One audit. Proof it works, and a testimonial." },
    { m: "Month 3", d: "Three audits, first retainer. Repeatable process." },
    { m: "Month 6", d: "Five retainers. Referrals start replacing cold outreach." },
    { m: "Month 12", d: "Ten retainers. Now it needs a second person delivering." },
  ];
  miles.forEach((mi, i) => {
    const y = 1.72 + i * 1.02;
    s.addShape(pres.ShapeType.roundRect, {
      x: 8.55, y, w: 4.08, h: 0.86, rectRadius: 0.07,
      fill: { color: "12283C" }, line: { color: SLATE, width: 1 },
    });
    s.addText(mi.m, { x: 8.78, y: y + 0.1, w: 3.6, h: 0.28, fontFace: B, fontSize: 12.5, bold: true, color: GREEN, margin: 0 });
    s.addText(mi.d, { x: 8.78, y: y + 0.37, w: 3.66, h: 0.42, fontFace: B, fontSize: 10.5, color: "B4C6D6", lineSpacing: 14, margin: 0 });
  });

  s.addText(
    "The only thing standing between month one and month twelve is the number of conversations we start this week.",
    { x: M, y: 6.28, w: CW, h: 0.45, fontFace: B, fontSize: 15, bold: true, italic: true, color: GREEN, align: "center", margin: 0 }
  );

  s.addNotes(
    "Be honest about what this chart is and is not. It is not a forecast — it is arithmetic showing what " +
    "happens if we keep enough conversations running. Every number on it is a count of customers, not a " +
    "growth rate we are hoping for. " +
    "The critical structural point is the shape of the curve. Month one is one audit and almost no money, " +
    "and that is fine and expected — its purpose is proof, not profit. The steepening later comes almost " +
    "entirely from retainers stacking on top of each other, because each one keeps paying while we go and " +
    "win the next. " +
    "Month twelve is where this stops being four people with a side project and becomes a company that " +
    "needs a second delivery person. Do not plan past that yet. " +
    "Close on the final line, and mean it. Nothing on this slide is limited by our technical ability. " +
    "It is limited entirely by outreach volume — by how many conversations we are willing to start. " +
    "That is the only real constraint in this business, and it is completely within our control."
  );
}

pres.writeFile({ fileName: "/home/user/Blaze/finops-venture/FinOps-Business-Model.pptx" })
  .then((f) => console.log("WROTE:", f));
