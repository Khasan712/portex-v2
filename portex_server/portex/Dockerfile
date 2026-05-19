# Use an official Python runtime as the base image
FROM python:3.11-slim

# Set environment variables to prevent Python from writing .pyc files and buffering logs
ENV PYTHONDONTWRITEBYTECODE=1
ENV PYTHONUNBUFFERED=1

# Set the working directory in the container
WORKDIR /portex

# Copy the requirements file and install dependencies
COPY requirements.txt /portex/
RUN pip3 install --upgrade pip
RUN pip install --no-cache-dir -r requirements.txt

# Copy the Django project code into the container
COPY . /app/
