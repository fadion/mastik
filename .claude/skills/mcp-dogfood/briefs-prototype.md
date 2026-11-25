# Prototype briefs

The pool and passes for a `prototypes` run (`SKILL.md`, *Prototypes*). Each brief has two halves
on one entry: what the setup pass builds (**Surface:**), and what the tester makes play on it
(**Prototype:**). `run-tester.ps1` splits an entry at `Prototype:`. A new brief should need
every pass below, and its surface should hold what its prototype needs: the screens, a
component with interaction variants, a data table and variables.

## Pool

1. Surface: a shop app at 390 wide: a product list screen, a product detail screen, a cart
   screen and an order confirmation screen; a product card component with Default, Hover and
   Pressed variants; a Products table of eight rows (Name, Price, Image, Category); variables
   Cart count (number) and Promo code (string); a light and a dark mode. Prototype: the list
   from the Products table, a tapped card opening its detail screen, Add to cart adding a row
   to a Cart table and raising Cart count, the cart listing the Cart table with a total, a
   promo field bound to Promo code with a conditional discount, checkout sliding to the
   confirmation, a dark-mode toggle.
2. Surface: a task manager at 1280 wide: a board screen, a task detail overlay, a new-task
   overlay and a settings screen; a task row component with Default, Hover and Done variants;
   a Tasks table of ten rows (Title, Owner, Due, Status); variables Filter (string) and Show
   done (boolean). Prototype: the board's lists filtered by Status, a tapped row opening the
   detail overlay, the new-task overlay adding a row, a checkbox switching a row to Done and
   editing its Status, a search field bound to Filter, Escape closing an overlay, a sticky
   header over a scrolling board.
3. Surface: a music player at 390 wide: a library screen, a now-playing screen and a queue
   bottom sheet; a track row component with Default, Playing and Pressed variants; a Tracks
   table of twelve rows (Title, Artist, Length, Cover); variables Playing (boolean), Volume
   (number) and Current track (string). Prototype: the library listing Tracks, a tapped row
   opening now-playing with its track, play and pause toggling Playing and the button's
   variant, a volume knob that follows a drag and snaps, the queue sheet sliding up and
   closing on a swipe down, smart animate between the mini player and now-playing.
4. Surface: a banking app at 390 wide: a sign-in screen, an accounts screen, a transfer screen
   and a receipt overlay; a keypad key component with Default and Pressed variants; an
   Accounts table of four rows and a Transactions table of fifteen; variables Amount (number),
   Signed in (boolean) and PIN (string). Prototype: a PIN pad appending to PIN and a
   conditional sign-in on the right code, the accounts list from Accounts with a sum, a
   transactions list sorted by date and filtered by account, the transfer guarded by a
   balance check, the receipt overlay after a delay, a pull of the list revealing a header by
   a scroll effect.
5. Surface: an onboarding and settings flow for a fitness app at 390 wide: three onboarding
   screens, a goals screen and a profile screen; a choice chip component with Off and On
   variants; a Goals table of six rows (Goal, Minutes, Icon); variables Step (number), Units
   (string) and Reminders (boolean). Prototype: swipes between the onboarding screens with
   progress dots following Step, chips bound to booleans, a goals list from the table with a
   chosen one opened on the profile, a units switch setting a mode, a reminders toggle, a skip
   link going straight to the profile, flows for first run and for returning users.

## The passes

1. **Screens and flows.** Link the screens: navigation, back, overlays opened and closed,
   their positions and backdrops, the transitions a real app would use. Add a flow for each
   way a person enters the app, with the starting values it needs.
2. **States and motion.** Make the component's variants answer hover and press, switch a
   state on a tap, use smart animate between screens that share layers, and set up the
   scrolling, pinned and sticky parts, a drag that snaps and a scroll effect where the brief
   asks for them.
3. **Data and logic.** Make the lists from the data tables with their filters and sorts, open
   a tapped row on a detail screen, add, edit and delete rows from interactions, bind inputs
   to variables, and use conditions and, where an action needs more than the actions give, a
   document script run by a person's act.
4. **Check and export.** Read the prototype back and walk each flow as a person would play it,
   fix every link that goes nowhere or does the wrong thing, then export the HTML file and the
   flow map.
