def ask(q; ctx):          judge({q: q}; ctx).value;
def choose(q; opts; ctx): judge({q: q, among: opts}; ctx).value;
def rate(q; levels; ctx): judge({q: q, levels: levels}; ctx).value;
def box(q; ctx):          judge({q: q, box: true}; ctx).value;
