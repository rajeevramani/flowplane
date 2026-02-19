"""Traffic generators for learning session tests.

These send real HTTP requests through the gateway to produce traffic
that the learning agent can discover and generate OpenAPI specs from.
"""
from __future__ import annotations

import httpx
import random
import string
import time
from dataclasses import dataclass
from datetime import datetime, timedelta


@dataclass
class TrafficResult:
    """Summary of traffic sent."""
    total_requests: int
    paths_exercised: set[str]
    methods_used: set[str]
    errors: list[str]


# ---------------------------------------------------------------------------
# MockBank data generators (adapted from mock-backend/traffic-generator.py)
# ---------------------------------------------------------------------------

FIRST_NAMES = ["John", "Jane", "Michael", "Sarah", "David", "Emily", "Robert", "Lisa", "James", "Maria"]
LAST_NAMES = ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Wilson", "Taylor"]
CITIES = ["New York", "Los Angeles", "Chicago", "Houston", "Phoenix", "Philadelphia", "San Antonio", "San Diego"]
STATES = ["NY", "CA", "IL", "TX", "AZ", "PA", "TX", "CA"]
STREETS = ["Main Street", "Oak Avenue", "Maple Drive", "Cedar Lane", "Pine Road", "Elm Street", "Park Avenue"]
MERCHANTS = ["Amazon", "Walmart", "Target", "Costco", "Home Depot", "Best Buy", "Starbucks"]
CATEGORIES = ["shopping", "groceries", "dining", "utilities", "entertainment", "travel", "healthcare"]
SYMBOLS = ["AAPL", "GOOGL", "MSFT", "AMZN", "TSLA", "META", "NVDA", "JPM", "V", "WMT"]
COMPANY_NAMES = ["Apple Inc.", "Alphabet Inc.", "Microsoft Corp.", "Amazon.com Inc.", "Tesla Inc."]


def _rand_str(n: int = 8) -> str:
    return "".join(random.choices(string.ascii_lowercase, k=n))


def _rand_email() -> str:
    return f"{_rand_str()}@example.com"


def _rand_phone() -> str:
    return f"+1-555-{random.randint(100,999)}-{random.randint(1000,9999)}"


def _rand_date(days_back: int = 365) -> str:
    d = datetime.now() - timedelta(days=random.randint(0, days_back))
    return d.strftime("%Y-%m-%d")


def _rand_future_date(days_ahead: int = 365) -> str:
    d = datetime.now() + timedelta(days=random.randint(1, days_ahead))
    return d.strftime("%Y-%m-%d")


def _rand_datetime(days_back: int = 30) -> str:
    dt = datetime.now() - timedelta(days=random.randint(0, days_back), hours=random.randint(0, 23))
    return dt.strftime("%Y-%m-%dT%H:%M:%SZ")


def _rand_acct_num() -> str:
    return f"{random.randint(1000,9999)}-{random.randint(1000,9999)}-{random.randint(1000,9999)}"


def _rand_amount(lo: float = 10.0, hi: float = 10000.0) -> float:
    return round(random.uniform(lo, hi), 2)


def _address() -> dict:
    idx = random.randint(0, len(CITIES) - 1)
    return {
        "street": f"{random.randint(1,999)} {random.choice(STREETS)}",
        "city": CITIES[idx],
        "state": STATES[idx],
        "zipCode": f"{random.randint(10000,99999)}",
        "country": "USA",
    }


def _coordinates() -> dict:
    return {
        "latitude": round(random.uniform(25.0, 48.0), 4),
        "longitude": round(random.uniform(-125.0, -70.0), 4),
    }


def _business_hours() -> dict:
    return {
        "monday": "9:00 AM - 5:00 PM",
        "tuesday": "9:00 AM - 5:00 PM",
        "wednesday": "9:00 AM - 5:00 PM",
        "thursday": "9:00 AM - 5:00 PM",
        "friday": "9:00 AM - 6:00 PM",
        "saturday": "10:00 AM - 2:00 PM",
        "sunday": "Closed",
    }


# -- per-resource payload generators --

def _gen_customer() -> dict:
    return {
        "firstName": random.choice(FIRST_NAMES),
        "lastName": random.choice(LAST_NAMES),
        "email": _rand_email(),
        "phone": _rand_phone(),
        "dateOfBirth": _rand_date(days_back=20000),
        "ssn": f"***-**-{random.randint(1000,9999)}",
        "address": _address(),
        "customerSince": _rand_date(days_back=2000),
        "status": random.choice(["active", "inactive", "suspended"]),
        "kycVerified": random.choice([True, False]),
    }


def _gen_account() -> dict:
    return {
        "customerId": random.randint(1, 10),
        "accountNumber": _rand_acct_num(),
        "accountType": random.choice(["checking", "savings", "money_market"]),
        "accountName": random.choice(["Primary Checking", "Savings Account", "Money Market"]),
        "balance": _rand_amount(100, 50000),
        "availableBalance": _rand_amount(100, 50000),
        "currency": "USD",
        "status": random.choice(["active", "inactive", "closed"]),
        "openedDate": _rand_date(days_back=2000),
        "interestRate": round(random.uniform(0.01, 5.0), 2),
        "overdraftLimit": _rand_amount(0, 1000),
        "minimumBalance": _rand_amount(0, 500),
    }


def _gen_transaction() -> dict:
    return {
        "accountId": random.randint(1, 10),
        "type": random.choice(["debit", "credit"]),
        "category": random.choice(CATEGORIES),
        "amount": _rand_amount(5, 500),
        "currency": "USD",
        "description": f"{random.choice(MERCHANTS)} Purchase",
        "merchantName": random.choice(MERCHANTS),
        "merchantCategory": random.choice(["retail", "online_retail", "restaurant", "grocery"]),
        "date": _rand_datetime(),
        "status": random.choice(["pending", "completed", "failed", "cancelled"]),
        "reference": f"TXN-{datetime.now().strftime('%Y%m%d')}{random.randint(10000,99999)}",
    }


def _gen_card() -> dict:
    card_type = random.choice(["debit", "credit"])
    r: dict = {
        "customerId": random.randint(1, 10),
        "accountId": random.randint(1, 10) if card_type == "debit" else None,
        "cardNumber": f"****-****-****-{random.randint(1000,9999)}",
        "cardType": card_type,
        "cardBrand": random.choice(["Visa", "Mastercard", "Amex", "Discover"]),
        "cardName": f"{random.choice(FIRST_NAMES)} {random.choice(LAST_NAMES)}",
        "expiryDate": f"{random.randint(1,12):02d}/{random.randint(2025,2030)}",
        "status": random.choice(["active", "inactive", "blocked", "expired"]),
        "dailyLimit": _rand_amount(1000, 10000),
        "monthlyLimit": _rand_amount(10000, 50000),
        "isContactless": random.choice([True, False]),
        "isVirtual": random.choice([True, False]),
        "issueDate": _rand_date(days_back=365),
    }
    if card_type == "credit":
        r["creditLimit"] = _rand_amount(5000, 50000)
        r["availableCredit"] = _rand_amount(1000, r["creditLimit"])
        r["currentBalance"] = _rand_amount(0, 5000)
        r["minimumPayment"] = _rand_amount(25, 500)
        r["paymentDueDate"] = _rand_future_date(30)
        r["apr"] = round(random.uniform(12.0, 25.0), 2)
    return r


def _gen_beneficiary() -> dict:
    return {
        "customerId": random.randint(1, 10),
        "name": f"{random.choice(FIRST_NAMES)} {random.choice(LAST_NAMES)}",
        "nickname": random.choice(["Mom", "Dad", "Landlord", "Utilities", "Friend"]),
        "accountNumber": f"****{random.randint(1000,9999)}",
        "bankName": random.choice(["Chase Bank", "Bank of America", "Wells Fargo", "Citibank"]),
        "routingNumber": f"{random.randint(100000000,999999999)}",
        "accountType": random.choice(["checking", "savings"]),
        "email": _rand_email(),
        "phone": _rand_phone(),
        "status": random.choice(["active", "inactive"]),
        "addedDate": _rand_date(days_back=500),
    }


def _gen_transfer() -> dict:
    t_type = random.choice(["internal", "external"])
    return {
        "customerId": random.randint(1, 10),
        "fromAccountId": random.randint(1, 10),
        "toAccountId": random.randint(1, 10) if t_type == "internal" else None,
        "beneficiaryId": random.randint(1, 5) if t_type == "external" else None,
        "amount": _rand_amount(50, 5000),
        "currency": "USD",
        "type": t_type,
        "description": random.choice(["Monthly savings", "Rent payment", "Bill payment", "Gift"]),
        "status": random.choice(["pending", "scheduled", "completed", "failed", "cancelled"]),
        "reference": f"TRF-{datetime.now().strftime('%Y%m%d')}{random.randint(10000,99999)}",
        "recurring": random.choice([True, False]),
        "frequency": random.choice(["daily", "weekly", "biweekly", "monthly", "quarterly", "annually"]),
    }


def _gen_loan() -> dict:
    principal = _rand_amount(5000, 500000)
    return {
        "customerId": random.randint(1, 10),
        "loanType": random.choice(["mortgage", "auto", "personal", "business"]),
        "loanName": random.choice(["Home Mortgage", "Auto Loan", "Personal Loan", "Business Loan"]),
        "principalAmount": principal,
        "currentBalance": _rand_amount(1000, principal),
        "interestRate": round(random.uniform(3.0, 15.0), 2),
        "monthlyPayment": _rand_amount(100, 5000),
        "termMonths": random.choice([36, 60, 120, 180, 240, 360]),
        "remainingMonths": random.randint(1, 360),
        "startDate": _rand_date(days_back=2000),
        "endDate": _rand_future_date(3650),
        "nextPaymentDate": _rand_future_date(30),
        "status": random.choice(["active", "paid_off", "defaulted", "deferred"]),
        "collateral": f"{random.randint(1,999)} {random.choice(STREETS)}, {random.choice(CITIES)}",
        "latePaymentFee": _rand_amount(25, 200),
        "autopayEnabled": random.choice([True, False]),
    }


def _gen_bill_payment() -> dict:
    return {
        "customerId": random.randint(1, 10),
        "payeeName": random.choice(["ConEdison", "Verizon", "AT&T", "Netflix", "Spotify"]),
        "payeeAccountNumber": f"****{random.randint(1000,9999)}",
        "category": random.choice(["utilities", "telecom", "streaming", "insurance"]),
        "amount": _rand_amount(20, 500),
        "frequency": random.choice(["once", "weekly", "biweekly", "monthly", "quarterly", "annually"]),
        "nextDueDate": _rand_future_date(30),
        "autopayEnabled": random.choice([True, False]),
        "fromAccountId": random.randint(1, 10),
        "status": random.choice(["active", "inactive", "cancelled"]),
    }


def _gen_notification() -> dict:
    return {
        "customerId": random.randint(1, 10),
        "type": random.choice(["transaction", "payment_due", "deposit", "security", "low_balance", "promotion"]),
        "title": random.choice(["Purchase Alert", "Payment Reminder", "Low Balance", "Security Alert"]),
        "message": f"A transaction of ${_rand_amount(10, 500)} was processed.",
        "date": _rand_datetime(),
        "read": random.choice([True, False]),
        "priority": random.choice(["low", "normal", "high"]),
    }


def _gen_branch() -> dict:
    return {
        "name": f"{random.choice(CITIES)} {random.choice(['Main', 'Downtown', 'Central'])} Branch",
        "address": f"{random.randint(1,999)} {random.choice(STREETS)}, {random.choice(CITIES)}",
        "phone": _rand_phone(),
        "email": f"branch{random.randint(1,100)}@mockbank.com",
        "hours": _business_hours(),
        "services": random.sample(["deposits", "withdrawals", "loans", "safe_deposit", "notary"], k=3),
        "hasATM": random.choice([True, False]),
        "hasDriveThru": random.choice([True, False]),
        "coordinates": _coordinates(),
    }


def _gen_atm() -> dict:
    return {
        "name": f"{random.choice(CITIES)} ATM {random.randint(1,100)}",
        "address": f"{random.randint(1,999)} {random.choice(STREETS)}, {random.choice(CITIES)}",
        "status": random.choice(["online", "offline", "maintenance"]),
        "features": random.sample(["cash_withdrawal", "deposits", "balance_inquiry", "transfers"], k=3),
        "is24Hours": random.choice([True, False]),
        "coordinates": _coordinates(),
    }


def _gen_exchange_rate() -> dict:
    currencies = ["USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF"]
    from_curr = random.choice(currencies)
    to_curr = random.choice([c for c in currencies if c != from_curr])
    return {
        "fromCurrency": from_curr,
        "toCurrency": to_curr,
        "rate": round(random.uniform(0.5, 150.0), 4),
        "lastUpdated": _rand_datetime(days_back=1),
    }


def _gen_investment() -> dict:
    return {
        "customerId": random.randint(1, 10),
        "accountType": random.choice(["brokerage", "ira", "401k", "roth_ira"]),
        "accountNumber": f"INV-{random.randint(100,999)}-{random.randint(1000,9999)}",
        "totalValue": _rand_amount(10000, 500000),
        "cashBalance": _rand_amount(1000, 20000),
        "contributionLimit": _rand_amount(6000, 23000),
        "contributedYTD": _rand_amount(0, 23000),
        "status": random.choice(["active", "inactive", "closed"]),
        "openedDate": _rand_date(days_back=2000),
    }


def _gen_holding() -> dict:
    symbol = random.choice(SYMBOLS)
    qty = random.randint(1, 500)
    avg_cost = _rand_amount(50, 500)
    cur_price = avg_cost * random.uniform(0.8, 1.5)
    mkt_val = qty * cur_price
    gain = mkt_val - (qty * avg_cost)
    return {
        "investmentId": random.randint(1, 5),
        "symbol": symbol,
        "name": random.choice(COMPANY_NAMES),
        "quantity": qty,
        "averageCost": round(avg_cost, 2),
        "currentPrice": round(cur_price, 2),
        "marketValue": round(mkt_val, 2),
        "gain": round(gain, 2),
        "gainPercent": round((gain / (qty * avg_cost)) * 100, 2),
    }


def _gen_bank_info() -> dict:
    return {
        "name": "MockBank Financial",
        "routingNumber": f"{random.randint(100000000,999999999)}",
        "swiftCode": f"MOCK{random.choice(['US','UK','EU'])}{random.randint(10,99)}",
        "customerService": "+1-800-555-BANK",
        "fraudHotline": "+1-800-555-FRAUD",
        "website": "https://www.mockbank.com",
    }


# resource name -> generator function
_GENERATORS: dict[str, callable] = {
    "customers": _gen_customer,
    "accounts": _gen_account,
    "transactions": _gen_transaction,
    "cards": _gen_card,
    "beneficiaries": _gen_beneficiary,
    "transfers": _gen_transfer,
    "loans": _gen_loan,
    "billPayments": _gen_bill_payment,
    "notifications": _gen_notification,
    "branches": _gen_branch,
    "atms": _gen_atm,
    "exchangeRates": _gen_exchange_rate,
    "investments": _gen_investment,
    "holdings": _gen_holding,
    "bankInfo": _gen_bank_info,
}

# All 16 resource types under /v2/api/
_RESOURCE_TYPES = list(_GENERATORS.keys())


# ---------------------------------------------------------------------------
# Public traffic generators
# ---------------------------------------------------------------------------

def send_mockbank_traffic(gateway_port: int, rounds: int = 1) -> TrafficResult:
    """Send MockBank Financial API traffic through the gateway.

    Exercises ~40+ endpoints across 16 resource types using full CRUD lifecycle:
    create -> list -> read by id -> update -> delete.
    Each round creates fresh resources so the learning agent sees every route pattern.
    """
    base = f"http://localhost:{gateway_port}"
    client = httpx.Client(timeout=15.0)
    paths: set[str] = set()
    methods: set[str] = set()
    errors: list[str] = []
    total = 0

    def _req(method: str, path: str, **kwargs) -> httpx.Response | None:
        nonlocal total
        total += 1
        paths.add(path)
        methods.add(method.upper())
        try:
            resp = getattr(client, method)(f"{base}{path}", **kwargs)
            return resp
        except httpx.HTTPError as e:
            errors.append(f"{method.upper()} {path}: {e}")
            return None

    for _round in range(rounds):
        for resource, gen_fn in _GENERATORS.items():
            res_path = f"/v2/api/{resource}"

            # 1. POST — create a resource
            payload = gen_fn()
            resp = _req("post", res_path, json=payload)

            # json-server returns the created object with an "id" field
            created_id = None
            if resp and resp.status_code in (200, 201):
                try:
                    created_id = resp.json().get("id")
                except Exception:
                    pass

            # 2. GET — list collection
            _req("get", res_path)

            # 3. GET — list with query params (pagination / filtering)
            _req("get", res_path, params={"_page": 1, "_limit": 5})

            # 4. GET — read single resource by id
            item_id = created_id if created_id else 1
            item_path = f"{res_path}/{item_id}"
            _req("get", item_path)

            # 5. PUT — full replace the resource
            update_payload = gen_fn()  # fresh payload for the update
            _req("put", item_path, json=update_payload)

            # 6. PATCH — partial update the resource
            # Pick a subset of fields for the partial update
            patch_payload = gen_fn()
            patch_keys = list(patch_payload.keys())[:3]  # first 3 fields
            _req("patch", item_path, json={k: patch_payload[k] for k in patch_keys})

            # 7. DELETE — remove the resource
            _req("delete", item_path)

    client.close()

    return TrafficResult(
        total_requests=total,
        paths_exercised=paths,
        methods_used=methods,
        errors=errors,
    )


def send_httpbin_traffic(gateway_port: int) -> TrafficResult:
    """Send basic httpbin traffic -- fallback for simpler tests."""
    base = f"http://localhost:{gateway_port}"
    client = httpx.Client(timeout=10.0)
    paths = set()
    methods = set()
    errors = []
    total = 0

    def _req(method: str, path: str, **kwargs):
        nonlocal total
        total += 1
        paths.add(path)
        methods.add(method.upper())
        try:
            getattr(client, method)(f"{base}{path}", **kwargs)
        except httpx.HTTPError as e:
            errors.append(f"{method.upper()} {path}: {e}")

    _req("get", "/get")
    _req("post", "/post", json={"key": "value"})
    _req("put", "/put", json={"key": "updated"})
    _req("delete", "/delete")
    _req("get", "/headers")
    _req("get", "/status/200")
    _req("get", "/status/404")
    _req("post", "/post", data="raw body", headers={"Content-Type": "text/plain"})

    client.close()
    return TrafficResult(total_requests=total, paths_exercised=paths, methods_used=methods, errors=errors)
